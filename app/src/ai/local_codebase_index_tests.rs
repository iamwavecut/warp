use std::path::PathBuf;
use std::str::FromStr;

use ::ai::api_keys::ApiKeys;
use ::ai::index::full_source_code_embedding::store_client::{IntermediateNode, StoreClient};
use ::ai::index::full_source_code_embedding::{
    ContentHash, EmbeddingConfig, Fragment, NodeHash, RepoMetadata,
};
use mockito::Matcher;
use serde_json::json;
use string_offset::ByteOffset;
use tempfile::TempDir;

use super::*;
use crate::ai::agent::api::direct_openai::resolve_custom_provider_embedding_route_with_readiness;
use crate::settings::{CustomApiType, CustomProviderCapabilities, CustomProviderConfig};

fn route(base_url: String, model: &str) -> CustomProviderRoute {
    CustomProviderRoute {
        provider_name: "local-embeddings".to_string(),
        base_url,
        model: model.to_string(),
        api_key: None,
        capabilities: CustomProviderCapabilities {
            embeddings: true,
            embedding_model: Some(model.to_string()),
            ..Default::default()
        },
    }
}

fn fragment(content: &str) -> Fragment {
    Fragment::from_byte_range(
        content.to_string(),
        ContentHash::from_content(content),
        PathBuf::from("/repo/src/lib.rs"),
        ByteOffset::from(0)..ByteOffset::from(content.len()),
    )
}

#[tokio::test]
async fn local_store_persists_merkle_nodes_and_vectors_across_restart() {
    let temp = TempDir::new().unwrap();
    let database_path = temp.path().join("warp.sqlite");
    let mut server = mockito::Server::new_async().await;
    let generation = server
        .mock("POST", "/v1/embeddings")
        .match_header("authorization", Matcher::Missing)
        .match_body(Matcher::PartialJson(json!({
            "model": "embed-local",
            "input": ["alpha parser", "beta renderer"]
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "data": [
                    {"index": 1, "embedding": [0.0, 1.0]},
                    {"index": 0, "embedding": [1.0, 0.0]}
                ]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let alpha = fragment("alpha parser");
    let beta = fragment("beta renderer");
    let alpha_hash = alpha.content_hash().clone();
    let beta_hash = beta.content_hash().clone();
    let root_hash = NodeHash::from_str(&"a".repeat(64)).unwrap();
    let store = LocalStoreClient::new(database_path.clone());
    store.set_test_route(route(server.url() + "/v1", "embed-local"));
    store
        .generate_embeddings(
            EmbeddingConfig::OpenAiTextSmall3_256,
            vec![alpha, beta],
            root_hash.clone(),
            RepoMetadata {
                path: Some("/repo".to_string()),
            },
        )
        .await
        .unwrap();
    store
        .update_intermediate_nodes(
            EmbeddingConfig::OpenAiTextSmall3_256,
            vec![IntermediateNode {
                hash: root_hash.clone(),
                children: vec![(&alpha_hash).into(), (&beta_hash).into()],
            }],
        )
        .await
        .unwrap();
    assert!(
        store
            .populate_merkle_tree_cache(
                EmbeddingConfig::OpenAiTextSmall3_256,
                root_hash.clone(),
                RepoMetadata {
                    path: Some("/repo".to_string()),
                },
            )
            .await
            .unwrap()
    );
    generation.assert_async().await;
    drop(store);

    let query = server
        .mock("POST", "/v1/embeddings")
        .match_header("authorization", Matcher::Missing)
        .match_body(Matcher::PartialJson(json!({
            "model": "embed-local",
            "input": ["parser"]
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"data": [{"index": 0, "embedding": [1.0, 0.0]}]}).to_string())
        .create_async()
        .await;
    let restored = LocalStoreClient::new(database_path);
    restored.set_test_route(route(server.url() + "/v1", "embed-local"));
    let relevant = restored
        .get_relevant_fragments(
            EmbeddingConfig::OpenAiTextSmall3_256,
            "parser".to_string(),
            root_hash.clone(),
            RepoMetadata {
                path: Some("/repo".to_string()),
            },
        )
        .await
        .unwrap();
    assert_eq!(relevant.first(), Some(&alpha_hash));
    assert!(
        restored
            .sync_merkle_tree(
                vec![root_hash, (&beta_hash).into()],
                EmbeddingConfig::OpenAiTextSmall3_256,
            )
            .await
            .unwrap()
            .is_empty()
    );
    query.assert_async().await;
}

#[tokio::test]
async fn route_change_uses_a_distinct_vector_space() {
    let temp = TempDir::new().unwrap();
    let root_hash = NodeHash::from_str(&"b".repeat(64)).unwrap();
    let store = LocalStoreClient::new(temp.path().join("warp.sqlite"));
    store.set_test_route(route("http://localhost:1234/v1".to_string(), "embed-a"));
    let mut connection = store.connection().unwrap();
    sql_query(
        "INSERT INTO local_codebase_index_nodes (space_id, node_hash, children_json) VALUES (?, ?, '[]')",
    )
    .bind::<Text, _>(route_space_id(&route(
        "http://localhost:1234/v1".to_string(),
        "embed-a",
    )))
    .bind::<Text, _>(root_hash.to_string())
    .execute(&mut connection)
    .unwrap();

    store.set_test_route(route("http://localhost:1234/v1".to_string(), "embed-b"));
    let missing = store
        .sync_merkle_tree(
            vec![root_hash.clone()],
            EmbeddingConfig::OpenAiTextSmall3_256,
        )
        .await
        .unwrap();
    assert_eq!(missing, HashSet::from([root_hash]));
}

#[test]
fn embeddings_route_requires_explicit_capability_and_model() {
    let provider = CustomProviderConfig {
        name: "local".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        models: vec!["chat".to_string()],
        api_type: CustomApiType::OpenAiCompatible,
        capabilities: CustomProviderCapabilities {
            embeddings: true,
            embedding_model: Some("embed-local".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let resolved = resolve_custom_provider_embedding_route_with_readiness(
        std::slice::from_ref(&provider),
        &ApiKeys::default(),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(resolved.model, "embed-local");
    assert!(resolved.effective_capabilities().embeddings);

    let missing_model = CustomProviderConfig {
        name: "legacy".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        capabilities: CustomProviderCapabilities {
            embeddings: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let resolved_after_legacy = resolve_custom_provider_embedding_route_with_readiness(
        &[missing_model, provider.clone()],
        &ApiKeys::default(),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(resolved_after_legacy.provider_name, "local");

    let mut disabled = provider;
    disabled.capabilities.embeddings = false;
    assert!(
        resolve_custom_provider_embedding_route_with_readiness(
            &[disabled],
            &ApiKeys::default(),
            true,
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn embeddings_response_validation_fails_closed() {
    let error = validate_embeddings_response(
        EmbeddingsResponse {
            data: vec![EmbeddingData {
                index: 0,
                embedding: vec![f32::NAN],
            }],
        },
        1,
    )
    .expect_err("non-finite vectors must be rejected");
    assert!(error.to_string().contains("invalid float vector"));
}
