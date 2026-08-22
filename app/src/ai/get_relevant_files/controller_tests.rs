use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use ai::index::build_outline;
use ai::index::locations::CodeContextLocation;
use tempfile::TempDir;

use crate::ai::get_relevant_files::api::FileContext;
use crate::ai::outline::OutlineStatus;

use super::{
    GetRelevantFilesError, LOCAL_CONTENT_SEARCH_MAX_BYTES_PER_FILE, LOCAL_CONTENT_SEARCH_MAX_FILES,
    LOCAL_OUTLINE_RESULT_LIMIT, local_outline_locations, local_outline_locations_from_candidates,
    rank_local_outline_candidates as rank_local_outline_candidates_with_content,
};

fn file(path: &str, symbols: &str) -> FileContext {
    FileContext {
        path: path.to_owned(),
        symbols: symbols.to_owned(),
    }
}

fn paths(files: Vec<FileContext>) -> Vec<String> {
    files.into_iter().map(|file| file.path).collect()
}

fn rank_local_outline_candidates(
    query: &str,
    partial_path_segments: Option<&[String]>,
    candidates: Vec<FileContext>,
) -> Vec<FileContext> {
    rank_local_outline_candidates_with_content(
        query,
        partial_path_segments,
        candidates,
        &HashMap::new(),
    )
}

fn write_file(root: &Path, relative_path: &str, contents: &[u8]) -> PathBuf {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

fn whole_file_paths(
    locations: &std::collections::HashSet<CodeContextLocation>,
) -> BTreeSet<PathBuf> {
    locations
        .iter()
        .map(|location| match location {
            CodeContextLocation::WholeFile(path) => path.clone(),
            CodeContextLocation::Fragment(_) => panic!("outline search returned a fragment"),
        })
        .collect()
}

#[test]
fn ranks_local_outline_candidates() {
    let partial_paths = vec!["src/auth".to_owned()];
    let ranked = rank_local_outline_candidates(
        "auth",
        Some(&partial_paths),
        vec![
            file("src/misc.rs", "fn superauthhelper"),
            file("src/http/handler.rs", "fn auth_handler"),
            file("src/other/authentication.rs", "fn unrelated"),
            file("src/auth/controller.rs", "fn unrelated"),
        ],
    );

    assert_eq!(
        paths(ranked),
        [
            "src/auth/controller.rs",
            "src/other/authentication.rs",
            "src/http/handler.rs",
            "src/misc.rs",
        ]
    );

    let ranked = rank_local_outline_candidates(
        "cache invalidation",
        None,
        vec![
            file("src/one.rs", "fn cache"),
            file("src/both.rs", "fn cache_invalidation"),
        ],
    );
    assert_eq!(paths(ranked), ["src/both.rs", "src/one.rs"]);

    let tied = rank_local_outline_candidates(
        "needle",
        None,
        vec![
            file("src/Zeta.rs", "fn needle"),
            file("src/alpha.rs", "fn needle"),
        ],
    );
    assert_eq!(paths(tied), ["src/alpha.rs", "src/Zeta.rs"]);
}

#[test]
fn ranks_local_outline_candidates_without_inventing_or_repeating_paths() {
    assert_eq!(
        rank_local_outline_candidates("needle", None, Vec::new()),
        Vec::<FileContext>::new()
    );
    assert_eq!(
        rank_local_outline_candidates("", None, vec![file("src/a.rs", ""), file("src/b.rs", "")],),
        Vec::<FileContext>::new()
    );
    assert_eq!(
        paths(rank_local_outline_candidates(
            "",
            None,
            vec![file("src/only.rs", "")],
        )),
        ["src/only.rs"]
    );

    let mut candidates = vec![
        file("src/duplicate.rs", "fn token"),
        file("src/duplicate.rs", "fn token"),
    ];
    candidates.extend(
        (0..=LOCAL_OUTLINE_RESULT_LIMIT)
            .map(|index| file(&format!("src/result_{index:02}.rs"), "fn token")),
    );
    let ranked = rank_local_outline_candidates("token", None, candidates);

    assert_eq!(ranked.len(), LOCAL_OUTLINE_RESULT_LIMIT);
    assert_eq!(
        ranked
            .iter()
            .filter(|file| file.path == "src/duplicate.rs")
            .count(),
        1
    );
    assert_eq!(ranked[0].path, "src/duplicate.rs");
    assert_eq!(
        ranked[LOCAL_OUTLINE_RESULT_LIMIT - 1].path,
        "src/result_18.rs"
    );
}

#[tokio::test]
async fn local_outline_search_returns_existing_whole_files_from_content_evidence() {
    let repo = TempDir::new().unwrap();
    let name_match = write_file(repo.path(), "src/needle_name.rs", b"fn unrelated() {}\n");
    let content_match = write_file(
        repo.path(),
        "src/content.rs",
        b"fn unrelated() { let search_value = \"needle\"; }\n",
    );
    write_file(repo.path(), "src/irrelevant.rs", b"fn unrelated() {}\n");
    let repo_root = dunce::canonicalize(repo.path()).unwrap();
    let outline = build_outline(&repo_root, Some(100)).await.unwrap();
    let outlined_paths = outline
        .to_file_symbols(None)
        .into_iter()
        .map(|file| file.path)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        outlined_paths,
        BTreeSet::from([
            "src/content.rs".to_owned(),
            "src/irrelevant.rs".to_owned(),
            "src/needle_name.rs".to_owned(),
        ])
    );
    let status = OutlineStatus::Complete(outline);

    let first =
        local_outline_locations(Some((&status, repo_root.clone())), "needle", None).unwrap();
    let second = local_outline_locations(Some((&status, repo_root)), "needle", None).unwrap();
    let expected = BTreeSet::from([
        dunce::canonicalize(name_match).unwrap(),
        dunce::canonicalize(content_match).unwrap(),
    ]);

    assert_eq!(whole_file_paths(&first), expected);
    assert_eq!(whole_file_paths(&second), expected);
    assert!(first.iter().all(|location| location.path().is_file()));
}

#[test]
fn local_outline_states_remain_explicit_errors() {
    let repo = TempDir::new().unwrap();
    let pending = OutlineStatus::Pending;
    let failed = OutlineStatus::Failed;

    assert!(matches!(
        local_outline_locations(Some((&pending, repo.path().to_path_buf())), "query", None,),
        Err(GetRelevantFilesError::Pending)
    ));
    assert!(matches!(
        local_outline_locations(Some((&failed, repo.path().to_path_buf())), "query", None,),
        Err(GetRelevantFilesError::CreateFailed)
    ));
    assert!(matches!(
        local_outline_locations(None, "query", None),
        Err(GetRelevantFilesError::Missing)
    ));
}

#[test]
fn search_codebase_local_content_search_obeys_file_and_byte_bounds() {
    let repo = TempDir::new().unwrap();
    let mut candidates = Vec::new();
    for index in 0..=LOCAL_CONTENT_SEARCH_MAX_FILES {
        let relative_path = format!("src/file_{index:03}.rs");
        let contents = if index + 1 >= LOCAL_CONTENT_SEARCH_MAX_FILES {
            b"// boundedneedle\n".as_slice()
        } else {
            b"// unrelated\n".as_slice()
        };
        write_file(repo.path(), &relative_path, contents);
        candidates.push(file(&relative_path, ""));
    }

    let locations =
        local_outline_locations_from_candidates(repo.path(), "boundedneedle", None, candidates);
    assert_eq!(
        whole_file_paths(&locations),
        BTreeSet::from([dunce::canonicalize(repo.path().join(format!(
            "src/file_{:03}.rs",
            LOCAL_CONTENT_SEARCH_MAX_FILES - 1
        )))
        .unwrap()])
    );

    let after_byte_bound = vec![b'x'; LOCAL_CONTENT_SEARCH_MAX_BYTES_PER_FILE];
    let mut after_byte_bound_with_match = after_byte_bound;
    after_byte_bound_with_match.extend_from_slice(b"boundedneedle");
    write_file(
        repo.path(),
        "src/after_byte_bound.rs",
        &after_byte_bound_with_match,
    );
    let locations = local_outline_locations_from_candidates(
        repo.path(),
        "boundedneedle",
        None,
        vec![file("src/after_byte_bound.rs", "")],
    );
    assert_eq!(locations.len(), 0);
}

#[test]
fn search_codebase_local_outline_rejects_missing_and_outside_paths() {
    let repo = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let valid = write_file(repo.path(), "src/valid.rs", b"// safequery\n");
    let outside_file = write_file(outside.path(), "outside.rs", b"// safequery\n");
    let candidates = vec![
        file("src/valid.rs", ""),
        file("src/missing.rs", "fn safequery"),
        file(&outside_file.to_string_lossy(), "fn safequery"),
    ];

    let locations =
        local_outline_locations_from_candidates(repo.path(), "safequery", None, candidates);

    assert_eq!(
        whole_file_paths(&locations),
        BTreeSet::from([dunce::canonicalize(valid).unwrap()])
    );
}
