use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ai::index::full_source_code_embedding::RetrievalID;
use ai::index::full_source_code_embedding::manager::{
    CodebaseIndexManager, CodebaseIndexManagerEvent,
};
use ai::index::locations::CodeContextLocation;
use anyhow::{Context as _, anyhow};
use futures_util::stream::AbortHandle;
use instant::Instant;
use unicode_normalization_alignments::UnicodeNormalization as _;
use warp_core::features::FeatureFlag;
use warp_errors::report_error;
use warpui::{AppContext, Entity, ModelContext, ModelHandle, SingletonEntity};

#[cfg(not(target_family = "wasm"))]
use crate::ai::agent::SearchCodebaseFailureReason;
use crate::ai::{
    agent::{AIAgentActionId, SearchCodebaseResult},
    blocklist::SessionContext,
    get_relevant_files::api::FileContext as FileContextRequest,
    outline::{OutlineStatus, RepoOutlines},
};
#[cfg_attr(not(target_family = "wasm"), path = "remote_search/native.rs")]
#[cfg_attr(target_family = "wasm", path = "remote_search/wasm.rs")]
mod remote_search;

const LOCAL_OUTLINE_RESULT_LIMIT: usize = 20;
const LOCAL_CONTENT_SEARCH_MAX_FILES: usize = 128;
const LOCAL_CONTENT_SEARCH_MAX_BYTES_PER_FILE: usize = 64 * 1024;

// Derived ordering is deliberately lexicographic: one match in a higher field dominates every
// possible sum of all lower fields, while counts within a field still let every query token
// contribute. Content evidence stays below every outline tier.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct LocalCandidateScore {
    path_or_partial_matches: u64,
    filename_matches: u64,
    symbol_token_or_prefix_matches: u64,
    symbol_substring_matches: u64,
    content_token_matches: u64,
    content_substring_matches: u64,
}

impl LocalCandidateScore {
    fn add_content(mut self, content: Self) -> Self {
        self.content_token_matches += content.content_token_matches;
        self.content_substring_matches += content.content_substring_matches;
        self
    }
}

fn normalized_tokens(value: &str) -> Vec<String> {
    let normalized = value
        .nfc()
        .map(|(character, _)| character)
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>();
    normalized
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect()
}

fn normalized_path(path: &str) -> String {
    normalized_tokens(path).join("/")
}

fn contains_token_sequence(haystack: &[String], needle: &[String]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn outline_candidate_score(
    query_tokens: &[String],
    partial_path_segments: Option<&[String]>,
    candidate: &FileContextRequest,
) -> LocalCandidateScore {
    let path = Path::new(&candidate.path);
    let path_tokens = normalized_tokens(&candidate.path);
    let directory_tokens = path
        .parent()
        .map(|parent| normalized_tokens(&parent.to_string_lossy()))
        .unwrap_or_default();
    let file_name_tokens = path
        .file_name()
        .map(|file_name| normalized_tokens(&file_name.to_string_lossy()))
        .unwrap_or_default();
    let file_stem_tokens = path
        .file_stem()
        .map(|file_stem| normalized_tokens(&file_stem.to_string_lossy()))
        .unwrap_or_default();
    let symbol_tokens = normalized_tokens(&candidate.symbols);

    let mut score = LocalCandidateScore {
        path_or_partial_matches: partial_path_segments
            .into_iter()
            .flatten()
            .filter(|partial_path| {
                contains_token_sequence(&path_tokens, &normalized_tokens(partial_path))
            })
            .count() as u64,
        ..LocalCandidateScore::default()
    };

    for query_token in query_tokens {
        if directory_tokens.iter().any(|token| token == query_token) {
            score.path_or_partial_matches += 1;
        } else if file_name_tokens
            .iter()
            .chain(&file_stem_tokens)
            .any(|token| token.contains(query_token))
        {
            score.filename_matches += 1;
        } else if symbol_tokens
            .iter()
            .any(|token| token == query_token || token.starts_with(query_token))
        {
            score.symbol_token_or_prefix_matches += 1;
        } else if symbol_tokens
            .iter()
            .any(|token| token.contains(query_token))
        {
            score.symbol_substring_matches += 1;
        }
    }
    score
}

fn rank_local_outline_candidates(
    query: &str,
    partial_path_segments: Option<&[String]>,
    candidates: Vec<FileContextRequest>,
    content_scores: &HashMap<String, LocalCandidateScore>,
) -> Vec<FileContextRequest> {
    let mut candidates = candidates;
    candidates.sort_by(|left, right| {
        normalized_path(&left.path)
            .cmp(&normalized_path(&right.path))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.symbols.cmp(&right.symbols))
    });
    candidates.dedup_by(|left, right| left.path == right.path);

    let query_tokens = normalized_tokens(query);
    if query_tokens.is_empty() {
        return if candidates.len() < 2 {
            candidates
        } else {
            Vec::new()
        };
    }

    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| {
            let score = outline_candidate_score(&query_tokens, partial_path_segments, &candidate)
                .add_content(
                    content_scores
                        .get(&candidate.path)
                        .copied()
                        .unwrap_or_default(),
                );
            (score != LocalCandidateScore::default()).then(|| {
                (
                    score,
                    normalized_path(&candidate.path),
                    candidate.path.clone(),
                    candidate,
                )
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(
        |(left_score, left_path, left_raw_path, _),
         (right_score, right_path, right_raw_path, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_path.cmp(right_path))
                .then_with(|| left_raw_path.cmp(right_raw_path))
        },
    );
    ranked
        .into_iter()
        .take(LOCAL_OUTLINE_RESULT_LIMIT)
        .map(|(_, _, _, candidate)| candidate)
        .collect()
}

#[derive(Debug)]
struct ValidatedLocalCandidate {
    context: FileContextRequest,
}

fn canonical_candidate_path(
    canonical_root: &Path,
    candidate: &FileContextRequest,
) -> Option<PathBuf> {
    let path = dunce::canonicalize(canonical_root.join(&candidate.path)).ok()?;
    (path.starts_with(canonical_root) && path.is_file()).then_some(path)
}

#[cfg(unix)]
fn opened_file_matches_path(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    let (Ok(opened), Ok(current)) = (file.metadata(), path.metadata()) else {
        return false;
    };
    opened.dev() == current.dev() && opened.ino() == current.ino()
}

#[cfg(not(unix))]
fn opened_file_matches_path(file: &File, path: &Path) -> bool {
    file.metadata().is_ok() && path.metadata().is_ok()
}

fn open_local_candidate(
    canonical_root: &Path,
    candidate: &FileContextRequest,
) -> Option<(File, PathBuf)> {
    // Canonicalization enforces containment. On Unix, O_NOFOLLOW rejects a last-component symlink;
    // the second canonicalization plus device/inode comparison also detects a path-component swap
    // that made the opened handle differ from the path currently selected for return.
    let path = canonical_candidate_path(canonical_root, candidate)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(&path).ok()?;

    let revalidated_path = canonical_candidate_path(canonical_root, candidate)?;
    (revalidated_path == path && opened_file_matches_path(&file, &revalidated_path))
        .then_some((file, revalidated_path))
}

fn revalidate_local_candidate(
    canonical_root: &Path,
    candidate: &FileContextRequest,
) -> Option<PathBuf> {
    open_local_candidate(canonical_root, candidate).map(|(_, path)| path)
}

fn validated_local_candidates(
    base_path: &Path,
    mut candidates: Vec<FileContextRequest>,
) -> Vec<ValidatedLocalCandidate> {
    let Ok(canonical_root) = dunce::canonicalize(base_path) else {
        return Vec::new();
    };
    candidates.sort_by(|left, right| {
        normalized_path(&left.path)
            .cmp(&normalized_path(&right.path))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.symbols.cmp(&right.symbols))
    });

    let mut by_canonical_path = BTreeMap::new();
    for context in candidates {
        let Some(absolute_path) = canonical_candidate_path(&canonical_root, &context) else {
            continue;
        };
        by_canonical_path
            .entry(absolute_path)
            .or_insert(ValidatedLocalCandidate { context });
    }
    by_canonical_path.into_values().collect()
}

fn content_candidate_score(query_tokens: &[String], contents: &[u8]) -> LocalCandidateScore {
    if contents.contains(&0) {
        return LocalCandidateScore::default();
    }
    let content_tokens = normalized_tokens(&String::from_utf8_lossy(contents));
    let mut score = LocalCandidateScore::default();
    for query_token in query_tokens {
        if content_tokens.iter().any(|token| token == query_token) {
            score.content_token_matches += 1;
        } else if content_tokens
            .iter()
            .any(|token| token.contains(query_token))
        {
            score.content_substring_matches += 1;
        }
    }
    score
}

/// Builds bounded local content evidence without invoking a shell or child process.
///
/// At most 128 already validated outline files are opened, in deterministic outline-score/path
/// order, and only the first 64 KiB of each file is read (at most 8 MiB per request).
fn bounded_content_scores(
    canonical_root: &Path,
    query: &str,
    partial_path_segments: Option<&[String]>,
    candidates: &[ValidatedLocalCandidate],
) -> HashMap<String, LocalCandidateScore> {
    let query_tokens = normalized_tokens(query);
    if query_tokens.is_empty() {
        return HashMap::new();
    }

    let mut search_order = candidates.iter().collect::<Vec<_>>();
    search_order.sort_by(|left, right| {
        outline_candidate_score(&query_tokens, partial_path_segments, &right.context)
            .cmp(&outline_candidate_score(
                &query_tokens,
                partial_path_segments,
                &left.context,
            ))
            .then_with(|| {
                normalized_path(&left.context.path).cmp(&normalized_path(&right.context.path))
            })
            .then_with(|| left.context.path.cmp(&right.context.path))
    });

    search_order
        .into_iter()
        .take(LOCAL_CONTENT_SEARCH_MAX_FILES)
        .filter_map(|candidate| {
            let (file, _) = open_local_candidate(canonical_root, &candidate.context)?;
            let mut contents = Vec::with_capacity(LOCAL_CONTENT_SEARCH_MAX_BYTES_PER_FILE);
            file.take(LOCAL_CONTENT_SEARCH_MAX_BYTES_PER_FILE as u64)
                .read_to_end(&mut contents)
                .ok()?;
            let score = content_candidate_score(&query_tokens, &contents);
            (score != LocalCandidateScore::default())
                .then(|| (candidate.context.path.clone(), score))
        })
        .collect()
}

fn local_outline_locations_from_candidates(
    base_path: &Path,
    query: &str,
    partial_path_segments: Option<&[String]>,
    candidates: Vec<FileContextRequest>,
) -> Arc<HashSet<CodeContextLocation>> {
    let Ok(canonical_root) = dunce::canonicalize(base_path) else {
        return Arc::new(HashSet::new());
    };
    let candidates = validated_local_candidates(&canonical_root, candidates);
    let content_scores =
        bounded_content_scores(&canonical_root, query, partial_path_segments, &candidates);
    let ranked = rank_local_outline_candidates(
        query,
        partial_path_segments,
        candidates
            .into_iter()
            .map(|candidate| candidate.context)
            .collect(),
        &content_scores,
    );

    Arc::new(
        ranked
            .into_iter()
            .filter_map(|candidate| revalidate_local_candidate(&canonical_root, &candidate))
            .map(CodeContextLocation::WholeFile)
            .collect(),
    )
}

fn local_outline_locations(
    outline: Option<(&OutlineStatus, PathBuf)>,
    query: &str,
    partial_path_segments: Option<&Vec<String>>,
) -> Result<Arc<HashSet<CodeContextLocation>>, GetRelevantFilesError> {
    match outline {
        Some((OutlineStatus::Complete(outline), base_path)) => {
            let candidates = outline
                .to_file_symbols(partial_path_segments)
                .into_iter()
                .map(|file| FileContextRequest {
                    path: file.path,
                    symbols: file.symbols,
                })
                .collect();
            Ok(local_outline_locations_from_candidates(
                &base_path,
                query,
                partial_path_segments.map(Vec::as_slice),
                candidates,
            ))
        }
        Some((OutlineStatus::Pending, _)) => Err(GetRelevantFilesError::Pending),
        Some((OutlineStatus::Failed, _)) => Err(GetRelevantFilesError::CreateFailed),
        None => Err(GetRelevantFilesError::Missing),
    }
}

#[derive(Debug)]
pub enum GetRelevantFilesControllerEvent {
    Success {
        action_id: AIAgentActionId,
        result: GetRelevantFilesControllerResult,
    },
    Error {
        action_id: AIAgentActionId,
    },
}

impl GetRelevantFilesControllerEvent {
    pub fn action_id(&self) -> &AIAgentActionId {
        match self {
            GetRelevantFilesControllerEvent::Success { action_id, .. } => action_id,
            GetRelevantFilesControllerEvent::Error { action_id } => action_id,
        }
    }
}

#[derive(Debug)]
pub enum GetRelevantFilesControllerResult {
    Locations(Arc<HashSet<CodeContextLocation>>),
    SearchResult(SearchCodebaseResult),
}

pub enum GetRelevantFilesRequestTarget {
    Local {
        directory: PathBuf,
    },
    Remote {
        session_context: SessionContext,
        requested_codebase_path: Option<String>,
    },
}
#[derive(Debug, thiserror::Error)]
pub enum GetRelevantFilesError {
    #[error("Repo outline is still being computed.")]
    Pending,
    #[error("Failed to create outline.")]
    CreateFailed,
    #[error("Failed to create outline.")]
    Missing,
}

/// This enum allows us to use both the existing structure for outline-based indexing
/// and the new full source code indexing manager/model.
enum RequestHandle {
    /// Used with outline-based indexing.
    AbortHandle(AbortHandle),

    /// Used with full source code indexing.
    RetrievalID {
        repo_path: PathBuf,
        retrieval_id: RetrievalID,
        start_time: Instant,
        fallback: LocalSearchFallback,
    },
}

#[derive(Clone)]
struct LocalSearchFallback {
    directory: PathBuf,
    query: String,
    partial_path_segments: Option<Vec<String>>,
}

impl RequestHandle {
    fn abort(&mut self, ctx: &mut AppContext) {
        match self {
            RequestHandle::AbortHandle(abort_handle) => abort_handle.abort(),
            RequestHandle::RetrievalID {
                repo_path,
                retrieval_id,
                ..
            } => {
                CodebaseIndexManager::handle(ctx).update(ctx, |index_manager, ctx| {
                    if let Err(err) = index_manager
                        .abort_retrieval_request(repo_path, retrieval_id.clone(), ctx)
                        .context("Failed to abort file retrieval request")
                    {
                        report_error!(err);
                    }
                });
            }
        }
    }
}

/// Controller for GetRelevantFiles action. This is scoped per terminal session.
#[derive(Default)]
pub struct GetRelevantFilesController {
    /// Search requests currently in flight, keyed by the originating action ID.
    /// This allows several SearchCodebase actions to be active at once without newer requests
    /// cancelling unrelated older ones.
    pending_requests: std::collections::HashMap<AIAgentActionId, RequestHandle>,
}

impl GetRelevantFilesController {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let codebase_manager = CodebaseIndexManager::handle(ctx);
        ctx.subscribe_to_model(&codebase_manager, Self::handle_codebase_manager_event);
        Self::default()
    }

    fn pending_request_details_for_retrieval_id(
        &self,
        pending_retrieval_id: &RetrievalID,
    ) -> Option<(AIAgentActionId, Instant, LocalSearchFallback)> {
        // Full-source embedding completion events only carry the retrieval ID, so map them back to
        // the agent action that initiated the request before emitting results/diagnostics.
        self.pending_requests
            .iter()
            .find_map(|(action_id, request_handle)| match request_handle {
                RequestHandle::AbortHandle(_) => None,
                RequestHandle::RetrievalID {
                    retrieval_id,
                    start_time,
                    fallback,
                    ..
                } if retrieval_id == pending_retrieval_id => {
                    Some((action_id.clone(), *start_time, fallback.clone()))
                }
                RequestHandle::RetrievalID { .. } => None,
            })
    }

    fn handle_codebase_manager_event(
        &mut self,
        _: ModelHandle<CodebaseIndexManager>,
        codebase_manager_event: &CodebaseIndexManagerEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match codebase_manager_event {
            CodebaseIndexManagerEvent::RetrievalRequestFailed {
                retrieval_id,
                error_message: error,
            } => {
                let Some((action_id, _search_start, fallback)) =
                    self.pending_request_details_for_retrieval_id(retrieval_id)
                else {
                    return;
                };
                self.finish_semantic_or_local_fallback(
                    Err(anyhow!(error.to_owned())),
                    action_id,
                    fallback,
                    ctx,
                );
            }
            CodebaseIndexManagerEvent::RetrievalRequestCompleted {
                retrieval_id,
                fragments,
                out_of_sync_delay: _,
            } => {
                let Some((action_id, _search_start, fallback)) =
                    self.pending_request_details_for_retrieval_id(retrieval_id)
                else {
                    return;
                };

                self.finish_semantic_or_local_fallback(
                    Ok(fragments.clone()),
                    action_id,
                    fallback,
                    ctx,
                );
            }
            _ => (),
        }
    }

    /// Start a new search query based on the repo outline.
    pub fn send_request(
        &mut self,
        target: GetRelevantFilesRequestTarget,
        query: String,
        partial_path_segments: Option<&Vec<String>>,
        action_id: AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), GetRelevantFilesError> {
        // Cancel any previous request for this action before dispatching to either the local or
        // remote implementation.
        self.cancel_request_for_action(&action_id, ctx);
        match target {
            GetRelevantFilesRequestTarget::Local { directory } => {
                self.send_local_request(&directory, query, partial_path_segments, action_id, ctx)
            }
            GetRelevantFilesRequestTarget::Remote {
                session_context,
                requested_codebase_path,
            } => self.send_remote_request(
                session_context,
                requested_codebase_path,
                query,
                partial_path_segments.cloned(),
                action_id,
                ctx,
            ),
        }
    }

    fn send_local_request(
        &mut self,
        directory: &Path,
        query: String,
        partial_path_segments: Option<&Vec<String>>,
        action_id: AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), GetRelevantFilesError> {
        if FeatureFlag::FullSourceCodeEmbedding.is_enabled() {
            let codebase_mgr = CodebaseIndexManager::handle(ctx);
            if let Some(base_path) = codebase_mgr.as_ref(ctx).root_path_for_codebase(directory) {
                match codebase_mgr.update(ctx, |index_manager, ctx| {
                    index_manager.retrieve_relevant_files(query.clone(), base_path.as_path(), ctx)
                }) {
                    Ok(retrieval_request_id) => {
                        log::info!("Using full source code embedding for search");
                        let search_start = Instant::now();
                        self.pending_requests.insert(
                            action_id,
                            RequestHandle::RetrievalID {
                                repo_path: base_path.clone(),
                                retrieval_id: retrieval_request_id,
                                start_time: search_start,
                                fallback: LocalSearchFallback {
                                    directory: directory.to_path_buf(),
                                    query,
                                    partial_path_segments: partial_path_segments.cloned(),
                                },
                            },
                        );

                        return Ok(());
                    }
                    Err(e) => {
                        log::info!(
                            "Failed to initiate full source code search: {e}, falling back to outline-based search"
                        );
                    }
                }
            }
        }

        let locations = local_outline_locations(
            RepoOutlines::as_ref(ctx).get_outline(directory),
            &query,
            partial_path_segments,
        )?;
        ctx.emit(GetRelevantFilesControllerEvent::Success {
            action_id,
            result: GetRelevantFilesControllerResult::Locations(locations),
        });
        Ok(())
    }

    fn send_remote_request(
        &mut self,
        session_context: SessionContext,
        requested_codebase_path: Option<String>,
        query: String,
        partial_path_segments: Option<Vec<String>>,
        action_id: AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), GetRelevantFilesError> {
        match remote_search::send_request(
            query,
            partial_path_segments,
            session_context,
            requested_codebase_path,
            action_id.clone(),
            ctx,
        ) {
            #[cfg(not(target_family = "wasm"))]
            remote_search::RemoteSearchRequest::Pending(abort_handle) => {
                self.pending_requests
                    .insert(action_id, RequestHandle::AbortHandle(abort_handle));
            }
            remote_search::RemoteSearchRequest::Ready(result) => {
                ctx.emit(GetRelevantFilesControllerEvent::Success {
                    action_id,
                    result: GetRelevantFilesControllerResult::SearchResult(result),
                });
            }
        }
        Ok(())
    }

    fn finish_semantic_or_local_fallback(
        &mut self,
        relevant_file_locations: anyhow::Result<Arc<HashSet<CodeContextLocation>>>,
        action_id: AIAgentActionId,
        fallback: LocalSearchFallback,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.pending_requests.remove(&action_id).is_none() {
            return;
        }
        match relevant_file_locations {
            Ok(relevant_file_locations) if !relevant_file_locations.is_empty() => {
                ctx.emit(GetRelevantFilesControllerEvent::Success {
                    action_id,
                    result: GetRelevantFilesControllerResult::Locations(relevant_file_locations),
                });
            }
            semantic_result => {
                if let Err(error) = semantic_result {
                    log::warn!(
                        "Local semantic code search failed; using lexical fallback: {error:#}"
                    );
                }
                match local_outline_locations(
                    RepoOutlines::as_ref(ctx).get_outline(&fallback.directory),
                    &fallback.query,
                    fallback.partial_path_segments.as_ref(),
                ) {
                    Ok(locations) => ctx.emit(GetRelevantFilesControllerEvent::Success {
                        action_id,
                        result: GetRelevantFilesControllerResult::Locations(locations),
                    }),
                    Err(error) => {
                        report_error!(anyhow!(error).context("local lexical code search failed"));
                        ctx.emit(GetRelevantFilesControllerEvent::Error { action_id });
                    }
                }
            }
        };
    }

    #[cfg(not(target_family = "wasm"))]
    fn handle_remote_search_result(
        &mut self,
        search_result: anyhow::Result<SearchCodebaseResult>,
        action_id: AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.pending_requests.remove(&action_id).is_none() {
            return;
        }

        let result = search_result.unwrap_or_else(|e| SearchCodebaseResult::Failed {
            reason: SearchCodebaseFailureReason::ClientError,
            message: e.to_string(),
        });
        ctx.emit(GetRelevantFilesControllerEvent::Success {
            action_id,
            result: GetRelevantFilesControllerResult::SearchResult(result),
        });
    }

    /// Returns the path to the root directory for a codebase search where pwd is `directory`.
    pub fn root_directory_for_search(&self, directory: &Path, app: &AppContext) -> Option<PathBuf> {
        let mut start = None;
        if FeatureFlag::FullSourceCodeEmbedding.is_enabled() {
            start = CodebaseIndexManager::as_ref(app).root_path_for_codebase(directory);
        }
        start.or_else(|| {
            RepoOutlines::as_ref(app)
                .get_outline(directory)
                .map(|(_, root)| root)
        })
    }

    pub fn root_directory_for_remote_search(
        &self,
        session_context: &SessionContext,
        requested_codebase_path: Option<&str>,
        app: &AppContext,
    ) -> Option<PathBuf> {
        remote_search::root_directory_for_search(session_context, requested_codebase_path, app)
    }

    pub fn cancel_request_for_action(
        &mut self,
        action_id: &AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) {
        if let Some(mut request_handle) = self.pending_requests.remove(action_id) {
            request_handle.abort(ctx);
        }
    }
}

impl Entity for GetRelevantFilesController {
    type Event = GetRelevantFilesControllerEvent;
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
