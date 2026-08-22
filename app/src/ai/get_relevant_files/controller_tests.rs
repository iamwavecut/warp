use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ai::index::build_outline;
use ai::index::locations::CodeContextLocation;
use repo_metadata::DirectoryWatcher;
use tempfile::TempDir;
use warp_core::features::FeatureFlag;
use warp_util::standardized_path::StandardizedPath;
use warpui::{App, SingletonEntity};

use crate::ai::agent::AIAgentActionId;
use crate::ai::get_relevant_files::api::FileContext;
use crate::ai::outline::{OutlineStatus, RepoOutlines, insert_outline_for_test};

use super::{
    GetRelevantFilesController, GetRelevantFilesControllerEvent, GetRelevantFilesControllerResult,
    GetRelevantFilesError, GetRelevantFilesRequestTarget, LOCAL_CONTENT_SEARCH_MAX_BYTES_PER_FILE,
    LOCAL_CONTENT_SEARCH_MAX_FILES, LOCAL_OUTLINE_RESULT_LIMIT, LocalCandidateScore,
    local_outline_locations, local_outline_locations_from_candidates,
    rank_local_outline_candidates as rank_local_outline_candidates_with_content,
    revalidate_local_candidate,
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

#[test]
fn ranks_local_outline_candidates_without_cross_tier_inversion() {
    let query = "one two three four five six seven eight nine ten eleven twelve";
    let partial_paths = vec!["src/high".to_owned()];
    let path_match = file("src/high/unrelated.rs", "");
    let many_filename_matches = file(
        "src/one-two-three-four-five-six-seven-eight-nine-ten-eleven-twelve.rs",
        "",
    );
    assert_eq!(
        paths(rank_local_outline_candidates(
            query,
            Some(&partial_paths),
            vec![many_filename_matches, path_match],
        )),
        [
            "src/high/unrelated.rs",
            "src/one-two-three-four-five-six-seven-eight-nine-ten-eleven-twelve.rs",
        ]
    );

    let filename_match = file("src/one.rs", "");
    let many_symbol_matches = file(
        "src/symbols.rs",
        "fn two three four five six seven eight nine ten eleven twelve",
    );
    assert_eq!(
        paths(rank_local_outline_candidates(
            query,
            None,
            vec![many_symbol_matches, filename_match],
        )),
        ["src/one.rs", "src/symbols.rs"]
    );

    let symbol_token_match = file("src/symbol_token.rs", "fn one");
    let many_symbol_substrings = file(
        "src/symbol_substrings.rs",
        "fn xtwox xthreex xfourx xfivex xsixx xsevenx xeightx xninex xtenx xelevenx xtwelvex",
    );
    assert_eq!(
        paths(rank_local_outline_candidates(
            query,
            None,
            vec![many_symbol_substrings, symbol_token_match],
        )),
        ["src/symbol_token.rs", "src/symbol_substrings.rs"]
    );

    let symbol_substring = file("src/symbol_substring.rs", "fn prefixneedle_suffix");
    let content_only = file("src/content_only.rs", "");
    let content_scores = HashMap::from([(
        content_only.path.clone(),
        LocalCandidateScore {
            content_token_matches: 100,
            ..LocalCandidateScore::default()
        },
    )]);
    assert_eq!(
        paths(rank_local_outline_candidates_with_content(
            "needle",
            None,
            vec![content_only, symbol_substring],
            &content_scores,
        )),
        ["src/symbol_substring.rs", "src/content_only.rs"]
    );
}

#[test]
fn ranks_local_outline_candidates_with_canonically_equivalent_unicode() {
    let decomposed_query = "cafe\u{301}";
    let ranked = rank_local_outline_candidates(
        decomposed_query,
        None,
        vec![file("src/caf\u{e9}.rs", "fn unrelated")],
    );

    assert_eq!(paths(ranked), ["src/caf\u{e9}.rs"]);
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
fn local_outline_controller_dispatch_is_synchronous_and_keeps_no_pending_request() {
    App::test((), |mut app| async move {
        let repo = TempDir::new().unwrap();
        let matching = write_file(
            repo.path(),
            "src/controller_needle.rs",
            b"fn unrelated() {}\n",
        );
        write_file(repo.path(), "src/irrelevant.rs", b"fn unrelated() {}\n");
        let repo_root = dunce::canonicalize(repo.path()).unwrap();
        let outline = build_outline(&repo_root, Some(100)).await.unwrap();

        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repository = DirectoryWatcher::handle(&app).update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    StandardizedPath::from_local_canonicalized(&repo_root).unwrap(),
                    ctx,
                )
                .unwrap()
        });
        let outlines = app.add_singleton_model(RepoOutlines::new_for_test);
        outlines.update(&mut app, |outlines, _| {
            insert_outline_for_test(
                outlines,
                repo_root.clone(),
                repository,
                OutlineStatus::Complete(outline),
            );
        });

        let _embedding_flag = FeatureFlag::FullSourceCodeEmbedding.override_enabled(false);
        let controller = app.add_model(|_| GetRelevantFilesController::default());
        let observed = Rc::new(RefCell::new(Vec::new()));
        app.update(|ctx| {
            let observed = observed.clone();
            ctx.subscribe_to_model(&controller, move |_, event, _| {
                if let GetRelevantFilesControllerEvent::Success {
                    action_id,
                    result: GetRelevantFilesControllerResult::Locations(locations),
                } = event
                {
                    observed
                        .borrow_mut()
                        .push((action_id.clone(), whole_file_paths(locations)));
                }
            });
        });

        let action_id = AIAgentActionId::from("local-controller-search".to_owned());
        controller.update(&mut app, |controller, ctx| {
            controller
                .send_request(
                    GetRelevantFilesRequestTarget::Local {
                        directory: repo_root,
                    },
                    "controller needle".to_owned(),
                    None,
                    action_id.clone(),
                    ctx,
                )
                .unwrap();
            assert!(controller.pending_requests.is_empty());
        });

        assert_eq!(
            observed.borrow().as_slice(),
            [(
                action_id,
                BTreeSet::from([dunce::canonicalize(matching).unwrap()]),
            )]
        );
    });
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

#[test]
fn search_codebase_local_outline_revalidates_a_selected_path() {
    let repo = TempDir::new().unwrap();
    let candidate = file("src/replace_me.rs", "fn safequery");
    let candidate_path = write_file(repo.path(), &candidate.path, b"// safequery\n");
    let canonical_root = dunce::canonicalize(repo.path()).unwrap();

    assert!(revalidate_local_candidate(&canonical_root, &candidate).is_some());
    fs::remove_file(&candidate_path).unwrap();
    assert_eq!(
        revalidate_local_candidate(&canonical_root, &candidate),
        None
    );

    #[cfg(unix)]
    {
        let outside = TempDir::new().unwrap();
        let outside_file = write_file(outside.path(), "outside.rs", b"// safequery\n");
        std::os::unix::fs::symlink(outside_file, candidate_path).unwrap();
        assert_eq!(
            revalidate_local_candidate(&canonical_root, &candidate),
            None
        );
    }
}
