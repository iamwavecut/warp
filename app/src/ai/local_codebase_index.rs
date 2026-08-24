use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use ::ai::api_keys::ApiKeyManager;
use ::ai::index::full_source_code_embedding::store_client::{IntermediateNode, StoreClient};
use ::ai::index::full_source_code_embedding::{
    CodebaseContextConfig, ContentHash, EmbeddingConfig, Error as IndexError, Fragment, NodeHash,
    RepoMetadata,
};
use async_trait::async_trait;
use diesel::connection::SimpleConnection;
use diesel::sql_types::{Binary, Text};
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection, sql_query};
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use warpui::{AppContext, SingletonEntity};

use crate::ai::agent::api::direct_openai::{
    CustomProviderRoute, embeddings_url, resolve_custom_provider_embedding_route_with_readiness,
};
use crate::settings::AISettings;

const EMBEDDING_CADENCE: Duration = Duration::from_secs(5 * 60);
const MAX_RELEVANT_FRAGMENTS: usize = 64;
const MAX_EMBEDDING_DIMENSIONS: usize = 16_384;

const LOCAL_INDEX_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_codebase_index_nodes (
    space_id TEXT NOT NULL,
    node_hash TEXT NOT NULL,
    children_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (space_id, node_hash)
);
CREATE TABLE IF NOT EXISTS local_codebase_index_chunks (
    space_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    content TEXT NOT NULL,
    vector BLOB NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (space_id, content_hash)
);
CREATE TABLE IF NOT EXISTS local_codebase_index_roots (
    space_id TEXT NOT NULL,
    root_hash TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (space_id, root_hash)
);
CREATE INDEX IF NOT EXISTS local_codebase_index_roots_repo
    ON local_codebase_index_roots(repo_path, space_id);
"#;

#[derive(Clone)]
struct ReadyRoute {
    route: CustomProviderRoute,
    space_id: String,
    operational_id: String,
}

enum RouteState {
    Disabled(String),
    Ready(ReadyRoute),
}

impl RouteState {
    fn operational_id(&self) -> Option<&str> {
        match self {
            Self::Disabled(_) => None,
            Self::Ready(route) => Some(&route.operational_id),
        }
    }
}

/// Durable local implementation of the upstream full-source embedding store.
/// Only source chunks, content-addressed Merkle nodes, and float vectors are
/// stored. Credentials remain in memory and are never written to this store.
pub(crate) struct LocalStoreClient {
    database_path: PathBuf,
    http: Client,
    route: RwLock<RouteState>,
}

impl LocalStoreClient {
    pub(crate) fn new(database_path: PathBuf) -> Self {
        Self {
            database_path,
            http: Client::new(),
            route: RwLock::new(RouteState::Disabled(
                "No custom provider with embeddings is configured.".to_string(),
            )),
        }
    }

    pub(crate) fn for_current_scope() -> Self {
        Self::new(crate::persistence::database_file_path_for_current_scope())
    }

    /// Refresh the in-memory route from local settings and secure storage.
    /// Returns true when retrying/reindexing may now produce a different result.
    pub(crate) fn refresh_route(&self, app: &AppContext) -> bool {
        let settings = AISettings::as_ref(app);
        let keys = ApiKeyManager::as_ref(app);
        let route = resolve_custom_provider_embedding_route_with_readiness(
            &settings.custom_providers,
            keys.keys(),
            keys.keys_ready(),
        );
        self.set_route_result(route.map_err(|error| error.to_string()))
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(*self.route.read(), RouteState::Ready(_))
    }

    fn set_route_result(&self, route: Result<Option<CustomProviderRoute>, String>) -> bool {
        let next = match route {
            Ok(Some(route)) if route.effective_capabilities().embeddings => {
                let space_id = route_space_id(&route);
                let operational_id = route_operational_id(&route);
                RouteState::Ready(ReadyRoute {
                    route,
                    space_id,
                    operational_id,
                })
            }
            Ok(Some(route)) => RouteState::Disabled(format!(
                "Custom provider `{}` does not have an effective embeddings capability.",
                route.provider_name
            )),
            Ok(None) => RouteState::Disabled(
                "No custom provider with embeddings and an embedding model is configured."
                    .to_string(),
            ),
            Err(error) => RouteState::Disabled(error),
        };

        let mut current = self.route.write();
        let changed = current.operational_id() != next.operational_id()
            || matches!((&*current, &next), (RouteState::Disabled(a), RouteState::Disabled(b)) if a != b);
        *current = next;
        changed
    }

    #[cfg(test)]
    fn set_test_route(&self, route: CustomProviderRoute) {
        self.set_route_result(Ok(Some(route)));
    }

    fn ready_route(&self) -> Result<ReadyRoute, IndexError> {
        match &*self.route.read() {
            RouteState::Ready(route) => Ok(route.clone()),
            RouteState::Disabled(reason) => Err(local_index_error(reason.clone())),
        }
    }

    fn connection(&self) -> Result<SqliteConnection, IndexError> {
        if let Some(parent) = self.database_path.parent() {
            std::fs::create_dir_all(parent).map_err(IndexError::Io)?;
        }
        let path = self.database_path.to_str().ok_or_else(|| {
            local_index_error("Local codebase index database path is not valid UTF-8")
        })?;
        let mut connection = SqliteConnection::establish(path)
            .map_err(|error| local_index_error(format!("opening local index database: {error}")))?;
        connection
            .batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
            .and_then(|_| connection.batch_execute(LOCAL_INDEX_SCHEMA))
            .map_err(|error| {
                local_index_error(format!("initializing local index database: {error}"))
            })?;
        Ok(connection)
    }

    async fn embed(
        &self,
        route: &ReadyRoute,
        input: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, IndexError> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let body = EmbeddingsRequest {
            model: &route.route.model,
            input: &input,
            encoding_format: "float",
        };
        let mut request = self
            .http
            .post(embeddings_url(&route.route.base_url))
            .json(&body);
        if let Some(api_key) = route
            .route
            .api_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
        {
            request = request.bearer_auth(api_key);
        }
        let response = request.send().await.map_err(|error| {
            local_index_error(format!(
                "OpenAI-compatible embeddings request failed: {error}"
            ))
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("failed to read error response: {error}"));
            let body = body.chars().take(4096).collect::<String>();
            return Err(local_index_error(format!(
                "OpenAI-compatible embeddings endpoint returned {status}: {body}"
            )));
        }
        let response: EmbeddingsResponse = response
            .json()
            .await
            .map_err(|error| local_index_error(format!("decoding embeddings response: {error}")))?;
        validate_embeddings_response(response, input.len())
    }

    fn save_root(
        connection: &mut SqliteConnection,
        route: &ReadyRoute,
        root_hash: &NodeHash,
        repo_metadata: &RepoMetadata,
    ) -> Result<(), IndexError> {
        let repo_path = repo_metadata.path.as_deref().unwrap_or_default();
        sql_query(
            "INSERT INTO local_codebase_index_roots (space_id, root_hash, repo_path, updated_at) \
             VALUES (?, ?, ?, unixepoch()) \
             ON CONFLICT(space_id, root_hash) DO UPDATE SET \
             repo_path = excluded.repo_path, updated_at = excluded.updated_at",
        )
        .bind::<Text, _>(&route.space_id)
        .bind::<Text, _>(root_hash.to_string())
        .bind::<Text, _>(repo_path)
        .execute(connection)
        .map_err(|error| local_index_error(format!("saving local index root: {error}")))?;
        Ok(())
    }

    fn stored_hashes(
        connection: &mut SqliteConnection,
        space_id: &str,
    ) -> Result<HashSet<String>, IndexError> {
        #[derive(QueryableByName)]
        struct HashRow {
            #[diesel(sql_type = Text)]
            hash: String,
        }

        let mut hashes = sql_query(
            "SELECT node_hash AS hash FROM local_codebase_index_nodes WHERE space_id = ?",
        )
        .bind::<Text, _>(space_id)
        .load::<HashRow>(connection)
        .map_err(|error| local_index_error(format!("reading local Merkle nodes: {error}")))?
        .into_iter()
        .map(|row| row.hash)
        .collect::<HashSet<_>>();
        hashes.extend(
            sql_query(
                "SELECT content_hash AS hash FROM local_codebase_index_chunks WHERE space_id = ?",
            )
            .bind::<Text, _>(space_id)
            .load::<HashRow>(connection)
            .map_err(|error| local_index_error(format!("reading local index chunks: {error}")))?
            .into_iter()
            .map(|row| row.hash),
        );
        Ok(hashes)
    }

    fn load_space(
        connection: &mut SqliteConnection,
        space_id: &str,
    ) -> Result<(HashMap<String, Vec<String>>, HashMap<String, Vec<f32>>), IndexError> {
        #[derive(QueryableByName)]
        struct NodeRow {
            #[diesel(sql_type = Text)]
            node_hash: String,
            #[diesel(sql_type = Text)]
            children_json: String,
        }
        #[derive(QueryableByName)]
        struct ChunkRow {
            #[diesel(sql_type = Text)]
            content_hash: String,
            #[diesel(sql_type = Binary)]
            vector: Vec<u8>,
        }

        let nodes = sql_query(
            "SELECT node_hash, children_json FROM local_codebase_index_nodes WHERE space_id = ?",
        )
        .bind::<Text, _>(space_id)
        .load::<NodeRow>(connection)
        .map_err(|error| local_index_error(format!("loading local Merkle tree: {error}")))?
        .into_iter()
        .map(|row| {
            let children =
                serde_json::from_str::<Vec<String>>(&row.children_json).map_err(|error| {
                    local_index_error(format!("decoding local Merkle children: {error}"))
                })?;
            Ok((row.node_hash, children))
        })
        .collect::<Result<HashMap<_, _>, IndexError>>()?;
        let chunks = sql_query(
            "SELECT content_hash, vector FROM local_codebase_index_chunks WHERE space_id = ?",
        )
        .bind::<Text, _>(space_id)
        .load::<ChunkRow>(connection)
        .map_err(|error| local_index_error(format!("loading local chunk vectors: {error}")))?
        .into_iter()
        .map(|row| Ok((row.content_hash, decode_vector(&row.vector)?)))
        .collect::<Result<HashMap<_, _>, IndexError>>()?;
        Ok((nodes, chunks))
    }
}

#[async_trait]
impl StoreClient for LocalStoreClient {
    async fn update_intermediate_nodes(
        &self,
        _embedding_config: EmbeddingConfig,
        nodes: Vec<IntermediateNode>,
    ) -> Result<HashMap<NodeHash, bool>, IndexError> {
        let route = self.ready_route()?;
        let mut connection = self.connection()?;
        connection
            .transaction::<_, anyhow::Error, _>(|connection| {
                for node in &nodes {
                    let children = node
                        .children
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>();
                    let children_json = serde_json::to_string(&children).map_err(|error| {
                        local_index_error(format!("encoding local Merkle children: {error}"))
                    })?;
                    sql_query(
                        "INSERT INTO local_codebase_index_nodes \
                         (space_id, node_hash, children_json, updated_at) \
                         VALUES (?, ?, ?, unixepoch()) \
                         ON CONFLICT(space_id, node_hash) DO UPDATE SET \
                         children_json = excluded.children_json, updated_at = excluded.updated_at",
                    )
                    .bind::<Text, _>(&route.space_id)
                    .bind::<Text, _>(node.hash.to_string())
                    .bind::<Text, _>(children_json)
                    .execute(connection)
                    .map_err(|error| {
                        local_index_error(format!("saving local Merkle node: {error}"))
                    })?;
                }
                Ok(())
            })
            .map_err(|error| {
                local_index_error(format!("committing local Merkle nodes: {error}"))
            })?;
        Ok(nodes.into_iter().map(|node| (node.hash, true)).collect())
    }

    async fn generate_embeddings(
        &self,
        _embedding_config: EmbeddingConfig,
        fragments: Vec<Fragment>,
        root_hash: NodeHash,
        repo_metadata: RepoMetadata,
    ) -> Result<HashMap<ContentHash, bool>, IndexError> {
        let route = self.ready_route()?;
        let mut unique = Vec::<(ContentHash, String)>::new();
        let mut seen = HashSet::new();
        for fragment in fragments {
            if seen.insert(fragment.content_hash().to_string()) {
                unique.push((
                    fragment.content_hash().clone(),
                    fragment.content().to_string(),
                ));
            }
        }
        let vectors = self
            .embed(
                &route,
                unique.iter().map(|(_, content)| content.clone()).collect(),
            )
            .await?;
        let mut connection = self.connection()?;
        connection
            .transaction::<_, anyhow::Error, _>(|connection| {
                for ((hash, content), vector) in unique.iter().zip(&vectors) {
                    sql_query(
                        "INSERT INTO local_codebase_index_chunks \
                         (space_id, content_hash, content, vector, updated_at) \
                         VALUES (?, ?, ?, ?, unixepoch()) \
                         ON CONFLICT(space_id, content_hash) DO UPDATE SET \
                         content = excluded.content, vector = excluded.vector, \
                         updated_at = excluded.updated_at",
                    )
                    .bind::<Text, _>(&route.space_id)
                    .bind::<Text, _>(hash.to_string())
                    .bind::<Text, _>(content)
                    .bind::<Binary, _>(encode_vector(vector))
                    .execute(connection)
                    .map_err(|error| {
                        local_index_error(format!("saving local chunk embedding: {error}"))
                    })?;
                }
                Self::save_root(connection, &route, &root_hash, &repo_metadata)?;
                Ok(())
            })
            .map_err(|error| local_index_error(format!("committing local embeddings: {error}")))?;
        Ok(unique.into_iter().map(|(hash, _)| (hash, true)).collect())
    }

    async fn populate_merkle_tree_cache(
        &self,
        _embedding_config: EmbeddingConfig,
        root_hash: NodeHash,
        repo_metadata: RepoMetadata,
    ) -> Result<bool, IndexError> {
        let route = self.ready_route()?;
        let mut connection = self.connection()?;
        let exists =
            Self::stored_hashes(&mut connection, &route.space_id)?.contains(&root_hash.to_string());
        if exists {
            Self::save_root(&mut connection, &route, &root_hash, &repo_metadata)?;
        }
        Ok(exists)
    }

    async fn sync_merkle_tree(
        &self,
        nodes: Vec<NodeHash>,
        _embedding_config: EmbeddingConfig,
    ) -> Result<HashSet<NodeHash>, IndexError> {
        let route = self.ready_route()?;
        let mut connection = self.connection()?;
        let stored = Self::stored_hashes(&mut connection, &route.space_id)?;
        Ok(nodes
            .into_iter()
            .filter(|hash| !stored.contains(&hash.to_string()))
            .collect())
    }

    async fn rerank_fragments(
        &self,
        query: String,
        fragments: Vec<Fragment>,
    ) -> Result<Vec<Fragment>, IndexError> {
        let query_tokens = lexical_tokens(&query);
        let mut scored = fragments
            .into_iter()
            .enumerate()
            .map(|(position, fragment)| {
                let content_tokens = lexical_tokens(fragment.content());
                let score = query_tokens
                    .iter()
                    .filter(|token| content_tokens.contains(*token))
                    .count();
                (score, position, fragment)
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        Ok(scored
            .into_iter()
            .map(|(_, _, fragment)| fragment)
            .collect())
    }

    async fn get_relevant_fragments(
        &self,
        _embedding_config: EmbeddingConfig,
        query: String,
        root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> Result<Vec<ContentHash>, IndexError> {
        let route = self.ready_route()?;
        let query_vector = self
            .embed(&route, vec![query])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| local_index_error("Embeddings endpoint returned no query vector"))?;
        let mut connection = self.connection()?;
        let (nodes, chunks) = Self::load_space(&mut connection, &route.space_id)?;
        let reachable = reachable_chunks(&root_hash.to_string(), &nodes, &chunks)?;
        let mut scored = reachable
            .into_iter()
            .filter_map(|hash| {
                let vector = chunks.get(&hash)?;
                cosine_similarity(&query_vector, vector).map(|score| (score, hash))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        scored
            .into_iter()
            .take(MAX_RELEVANT_FRAGMENTS)
            .map(|(_, hash)| ContentHash::from_str(&hash))
            .collect()
    }

    async fn codebase_context_config(&self) -> Result<CodebaseContextConfig, IndexError> {
        self.ready_route()?;
        Ok(CodebaseContextConfig {
            embedding_config: EmbeddingConfig::OpenAiTextSmall3_256,
            embedding_cadence: EMBEDDING_CADENCE,
        })
    }
}

#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

fn validate_embeddings_response(
    response: EmbeddingsResponse,
    expected: usize,
) -> Result<Vec<Vec<f32>>, IndexError> {
    if response.data.len() != expected {
        return Err(local_index_error(format!(
            "Embeddings endpoint returned {} vectors for {expected} inputs",
            response.data.len()
        )));
    }
    let mut ordered = vec![None; expected];
    let mut dimensions = None;
    for item in response.data {
        if item.index >= expected || ordered[item.index].is_some() {
            return Err(local_index_error(
                "Embeddings endpoint returned duplicate or out-of-range indices",
            ));
        }
        if item.embedding.is_empty()
            || item.embedding.len() > MAX_EMBEDDING_DIMENSIONS
            || item.embedding.iter().any(|value| !value.is_finite())
        {
            return Err(local_index_error(
                "Embeddings endpoint returned an invalid float vector",
            ));
        }
        if dimensions
            .replace(item.embedding.len())
            .is_some_and(|previous| previous != item.embedding.len())
        {
            return Err(local_index_error(
                "Embeddings endpoint returned inconsistent vector dimensions",
            ));
        }
        ordered[item.index] = Some(item.embedding);
    }
    ordered
        .into_iter()
        .map(|vector| {
            vector.ok_or_else(|| local_index_error("Embeddings response omitted an input index"))
        })
        .collect()
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>, IndexError> {
    if bytes.is_empty() || bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(local_index_error("Stored embedding vector is malformed"));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if vector.len() > MAX_EMBEDDING_DIMENSIONS || vector.iter().any(|value| !value.is_finite()) {
        return Err(local_index_error("Stored embedding vector is invalid"));
    }
    Ok(vector)
}

fn reachable_chunks(
    root: &str,
    nodes: &HashMap<String, Vec<String>>,
    chunks: &HashMap<String, Vec<f32>>,
) -> Result<HashSet<String>, IndexError> {
    let mut pending = VecDeque::from([root.to_string()]);
    let mut visited = HashSet::new();
    let mut reachable = HashSet::new();
    while let Some(hash) = pending.pop_front() {
        if !visited.insert(hash.clone()) {
            continue;
        }
        if chunks.contains_key(&hash) {
            reachable.insert(hash);
        } else if let Some(children) = nodes.get(&hash) {
            pending.extend(children.iter().cloned());
        } else {
            return Err(local_index_error(format!(
                "Local codebase index is missing Merkle node {hash}"
            )));
        }
    }
    Ok(reachable)
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (&left, &right) in left.iter().zip(right) {
        let left = f64::from(left);
        let right = f64::from(right);
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    (denominator > f64::EPSILON).then_some(dot / denominator)
}

fn lexical_tokens(value: &str) -> HashSet<String> {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn route_space_id(route: &CustomProviderRoute) -> String {
    digest_fields([
        route.provider_name.as_str(),
        route.base_url.trim_end_matches('/'),
        route.model.as_str(),
        "openai-compatible-f32-v1",
    ])
}

fn route_operational_id(route: &CustomProviderRoute) -> String {
    let space_id = route_space_id(route);
    let key_fingerprint = route
        .api_key
        .as_deref()
        .map(|key| digest_fields([key]))
        .unwrap_or_default();
    digest_fields([space_id.as_str(), key_fingerprint.as_str()])
}

fn digest_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    let mut digest = Sha256::new();
    for field in fields {
        digest.update((field.len() as u64).to_le_bytes());
        digest.update(field.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn local_index_error(message: impl Into<String>) -> IndexError {
    IndexError::Other(anyhow::anyhow!(message.into()))
}

#[cfg(test)]
#[path = "local_codebase_index_tests.rs"]
mod tests;
