#![allow(clippy::field_reassign_with_default)]

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Uri};
use opensearch_lite::{
    agent::{client::AgentClient, validation::AgentResponseWrapper},
    http::{request::Request, router},
    server::AppState,
    Config,
};
use serde_json::{json, Value};

async fn call(
    state: &AppState,
    method: Method,
    path: &str,
    body: Value,
) -> opensearch_lite::responses::Response {
    let body = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(serde_json::to_vec(&body).unwrap())
    };
    let mut headers = HeaderMap::new();
    if !body.is_empty() {
        headers.insert("content-type", HeaderValue::from_static("application/json"));
    }
    let request = Request::from_parts(method, path.parse::<Uri>().unwrap(), headers, body);
    router::handle(state.clone(), request).await
}

async fn bulk_call(
    state: &AppState,
    method: Method,
    path: &str,
    body: &'static str,
) -> opensearch_lite::responses::Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/x-ndjson"),
    );
    let request = Request::from_parts(
        method,
        path.parse::<Uri>().unwrap(),
        headers,
        Bytes::from(body),
    );
    router::handle(state.clone(), request).await
}

async fn raw_call(
    state: &AppState,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> opensearch_lite::responses::Response {
    let request = Request::from_parts(method, path.parse::<Uri>().unwrap(), headers, body);
    router::handle(state.clone(), request).await
}

fn ephemeral_state() -> AppState {
    let mut config = Config::default();
    config.ephemeral = true;
    AppState::new(config).unwrap()
}

#[tokio::test]
async fn root_info_is_opensearch_shaped() {
    let state = ephemeral_state();
    let response = call(&state, Method::GET, "/", Value::Null).await;

    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["version"]["distribution"], "opensearch");
    assert_eq!(body["version"]["number"], "3.6.0");
}

#[tokio::test]
async fn template_document_and_search_flow() {
    let state = ephemeral_state();

    let template = json!({
        "index_patterns": ["orders-*"],
        "template": {
            "mappings": {
                "properties": {
                    "customer_id": { "type": "keyword" },
                    "total": { "type": "double" }
                }
            }
        }
    });
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/_index_template/orders-template",
            template
        )
        .await
        .status,
        200
    );

    let doc = json!({ "customer_id": "c1", "status": "paid", "total": 42.5 });
    let response = call(&state, Method::PUT, "/orders-2026/_doc/1", doc).await;
    assert_eq!(response.status, 201);

    let get = call(&state, Method::GET, "/orders-2026/_doc/1", Value::Null).await;
    assert_eq!(get.status, 200);
    assert_eq!(get.body.unwrap()["_source"]["customer_id"], "c1");

    let search = call(
        &state,
        Method::POST,
        "/orders-2026/_search",
        json!({
            "query": {
                "bool": {
                    "filter": [
                        { "term": { "customer_id": "c1" } },
                        { "range": { "total": { "gte": 40 } } }
                    ]
                }
            }
        }),
    )
    .await;
    assert_eq!(search.status, 200);
    assert_eq!(search.body.unwrap()["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn bulk_continues_after_item_failure() {
    let state = ephemeral_state();
    call(&state, Method::PUT, "/items", json!({})).await;

    let response = bulk_call(
        &state,
        Method::POST,
        "/_bulk",
        r#"{"index":{"_index":"items","_id":"1"}}
{"name":"one"}
{"bad":{}}
{"index":{"_index":"items","_id":"2"}}
{"name":"two"}
"#,
    )
    .await;

    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["errors"], true);
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        call(&state, Method::GET, "/items/_doc/2", Value::Null)
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn strict_mode_rejects_best_effort_without_allowlist() {
    let mut config = Config::default();
    config.ephemeral = true;
    config.strict_compatibility = true;
    let state = AppState::new(config).unwrap();

    let response = call(&state, Method::GET, "/_cluster/health", Value::Null).await;
    assert_eq!(response.status, 501);
    assert_eq!(
        response.body.unwrap()["error"]["type"],
        "opensearch_lite_strict_compatibility_exception"
    );
}

#[tokio::test]
async fn durable_mode_replays_committed_documents() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();

    let state = AppState::new(config.clone()).unwrap();
    call(&state, Method::PUT, "/durable", json!({})).await;
    call(
        &state,
        Method::PUT,
        "/durable/_doc/1",
        json!({ "name": "replayed" }),
    )
    .await;
    drop(state);

    let replayed = AppState::new(config).unwrap();
    let response = call(&replayed, Method::GET, "/durable/_doc/1", Value::Null).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body.unwrap()["_source"]["name"], "replayed");
}

#[tokio::test]
async fn configured_agent_fallback_can_answer_read_request() {
    let mut config = Config::default();
    config.ephemeral = true;
    config.agent.endpoint = Some("http://127.0.0.1:11434/v1/chat/completions".to_string());
    let wrapper = AgentResponseWrapper {
        status: 200,
        headers: Default::default(),
        body: json!({ "ok": true }),
        confidence: 90,
        failure_reason: None,
        read_only: true,
    };
    let state = AppState::with_agent(config, AgentClient::static_response(wrapper)).unwrap();

    let response = call(&state, Method::GET, "/_plugins/_unknown_read", Value::Null).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body.unwrap()["ok"], true);
    assert_eq!(
        response
            .headers
            .get("x-opensearch-lite-tier")
            .map(String::as_str),
        Some("agent_fallback_eligible")
    );
}

#[tokio::test]
async fn mutating_post_routes_do_not_enter_agent_fallback() {
    let mut config = Config::default();
    config.ephemeral = true;
    config.agent.endpoint = Some("http://127.0.0.1:11434/v1/chat/completions".to_string());
    let wrapper = AgentResponseWrapper {
        status: 200,
        headers: Default::default(),
        body: json!({ "agent": "answered" }),
        confidence: 90,
        failure_reason: None,
        read_only: true,
    };
    let state = AppState::with_agent(config, AgentClient::static_response(wrapper)).unwrap();

    let response = call(
        &state,
        Method::POST,
        "/orders/_delete_by_query",
        json!({ "query": { "match_all": {} } }),
    )
    .await;

    assert_eq!(response.status, 501);
    assert_ne!(response.body.unwrap()["agent"], "answered");
}

#[tokio::test]
async fn known_write_routes_with_wrong_get_method_do_not_enter_agent_fallback() {
    let mut config = Config::default();
    config.ephemeral = true;
    config.agent.endpoint = Some("http://127.0.0.1:11434/v1/chat/completions".to_string());
    let wrapper = AgentResponseWrapper {
        status: 200,
        headers: Default::default(),
        body: json!({ "agent": "answered" }),
        confidence: 90,
        failure_reason: None,
        read_only: true,
    };
    let state = AppState::with_agent(config, AgentClient::static_response(wrapper)).unwrap();

    for path in ["/orders/_delete_by_query", "/_reindex"] {
        let response = call(&state, Method::GET, path, Value::Null).await;
        assert_eq!(response.status, 501);
        assert_ne!(response.body.unwrap()["agent"], "answered");
    }
}

#[tokio::test]
async fn document_route_shape_rejections_do_not_mutate() {
    let state = ephemeral_state();

    let auto_id_put = call(
        &state,
        Method::PUT,
        "/invalid-doc/_doc",
        json!({ "name": "bad" }),
    )
    .await;
    assert_eq!(auto_id_put.status, 501);
    assert_eq!(
        call(&state, Method::HEAD, "/invalid-doc", Value::Null)
            .await
            .status,
        404
    );

    let extra = call(
        &state,
        Method::POST,
        "/invalid-doc/_doc/1/extra",
        json!({ "name": "bad" }),
    )
    .await;
    assert_eq!(extra.status, 501);
    assert_eq!(
        call(&state, Method::GET, "/invalid-doc/_doc/1", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn unsupported_bulk_methods_do_not_mutate() {
    let state = ephemeral_state();
    let response = bulk_call(
        &state,
        Method::GET,
        "/_bulk",
        r#"{"index":{"_index":"items","_id":"1"}}
{"name":"one"}
"#,
    )
    .await;

    assert_eq!(response.status, 501);
    assert_eq!(
        call(&state, Method::GET, "/items/_doc/1", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn create_conflicts_do_not_overwrite_documents() {
    let state = ephemeral_state();
    assert_eq!(
        call(
            &state,
            Method::PUT,
            "/jobs/_doc/1",
            json!({ "claim": "first" })
        )
        .await
        .status,
        201
    );

    let conflict = call(
        &state,
        Method::POST,
        "/jobs/_create/1",
        json!({ "claim": "second" }),
    )
    .await;
    assert_eq!(conflict.status, 409);

    let get = call(&state, Method::GET, "/jobs/_doc/1", Value::Null).await;
    assert_eq!(get.body.unwrap()["_source"]["claim"], "first");
}

#[tokio::test]
async fn bulk_malformed_source_and_missing_index_do_not_mutate() {
    let state = ephemeral_state();
    let malformed = bulk_call(
        &state,
        Method::POST,
        "/_bulk",
        r#"{"index":{"_index":"items","_id":"1"}}
{"name":
{"index":{"_id":"2"}}
{"name":"missing-index"}
"#,
    )
    .await;

    assert_eq!(malformed.status, 200);
    let body = malformed.body.unwrap();
    assert_eq!(body["errors"], true);
    assert_eq!(body["items"][0]["index"]["status"], 400);
    assert_eq!(body["items"][1]["index"]["status"], 400);
    assert!(state.store.resolve_index("").is_none());
    assert_eq!(
        call(&state, Method::GET, "/items/_doc/1", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn bulk_rejects_non_object_metadata_and_malformed_action_lines() {
    let state = ephemeral_state();

    let non_object_meta = bulk_call(
        &state,
        Method::POST,
        "/items/_bulk",
        r#"{"index":null}
{"name":"bad"}
"#,
    )
    .await;
    assert_eq!(non_object_meta.status, 400);
    assert_eq!(
        call(&state, Method::GET, "/items/_doc/1", Value::Null)
            .await
            .status,
        404
    );

    let malformed_action = bulk_call(
        &state,
        Method::POST,
        "/_bulk",
        r#"{"index":{"_index":"items","_id":"2"}
{"name":"bad"}
"#,
    )
    .await;
    assert_eq!(malformed_action.status, 400);
    assert_eq!(
        call(&state, Method::GET, "/items/_doc/2", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn memory_limit_rejects_large_persistent_state() {
    let mut config = Config::default();
    config.ephemeral = true;
    config.memory_limit_bytes = 700;
    config.max_body_bytes = 2_000;
    let state = AppState::new(config).unwrap();

    let response = call(
        &state,
        Method::PUT,
        "/limited/_doc/1",
        json!({ "payload": "x".repeat(900) }),
    )
    .await;

    assert_eq!(response.status, 429);
    assert_eq!(
        call(&state, Method::HEAD, "/limited", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn update_failure_does_not_create_index_and_upsert_honors_document_limit() {
    let state = ephemeral_state();
    let missing = call(
        &state,
        Method::POST,
        "/missing/_update/1",
        json!({ "doc": { "name": "nope" } }),
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        call(&state, Method::HEAD, "/missing", Value::Null)
            .await
            .status,
        404
    );

    let mut config = Config::default();
    config.ephemeral = true;
    config.max_documents = 1;
    let limited = AppState::new(config).unwrap();
    assert_eq!(
        call(
            &limited,
            Method::PUT,
            "/docs/_doc/1",
            json!({ "name": "one" })
        )
        .await
        .status,
        201
    );
    let rejected = call(
        &limited,
        Method::POST,
        "/docs/_update/2",
        json!({ "doc": { "name": "two" }, "doc_as_upsert": true }),
    )
    .await;
    assert_eq!(rejected.status, 429);

    let mut config = Config::default();
    config.ephemeral = true;
    config.max_documents = 0;
    let no_docs = AppState::new(config).unwrap();
    let rejected_missing_index = call(
        &no_docs,
        Method::POST,
        "/new-docs/_update/1",
        json!({ "doc": { "name": "blocked" }, "doc_as_upsert": true }),
    )
    .await;
    assert_eq!(rejected_missing_index.status, 429);
    assert_eq!(
        call(&no_docs, Method::HEAD, "/new-docs", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn ids_query_and_optional_should_match_opensearch_shape() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/searchable/_doc/1",
        json!({ "status": "paid" }),
    )
    .await;

    let ids = call(
        &state,
        Method::POST,
        "/searchable/_search",
        json!({ "query": { "ids": { "values": ["1"] } } }),
    )
    .await;
    assert_eq!(ids.body.unwrap()["hits"]["total"]["value"], 1);

    let optional_should = call(
        &state,
        Method::POST,
        "/searchable/_search",
        json!({
            "query": {
                "bool": {
                    "filter": { "term": { "status": "paid" } },
                    "should": { "term": { "status": "refunded" } }
                }
            }
        }),
    )
    .await;
    assert_eq!(optional_should.body.unwrap()["hits"]["total"]["value"], 1);

    let minimum_should_match = call(
        &state,
        Method::POST,
        "/searchable/_search",
        json!({
            "query": {
                "bool": {
                    "filter": { "term": { "status": "paid" } },
                    "should": [
                        { "term": { "status": "paid" } },
                        { "term": { "status": "refunded" } }
                    ],
                    "minimum_should_match": 2
                }
            }
        }),
    )
    .await;
    assert_eq!(
        minimum_should_match.body.unwrap()["hits"]["total"]["value"],
        0
    );

    call(
        &state,
        Method::PUT,
        "/searchable/_doc/negative",
        json!({ "score": -2 }),
    )
    .await;
    let negative_range = call(
        &state,
        Method::POST,
        "/searchable/_search",
        json!({ "query": { "range": { "score": { "gt": -10 } } } }),
    )
    .await;
    assert_eq!(negative_range.body.unwrap()["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn explicit_missing_search_index_returns_not_found() {
    let state = ephemeral_state();

    let response = call(
        &state,
        Method::POST,
        "/missing-search/_search",
        json!({ "query": { "match_all": {} } }),
    )
    .await;

    assert_eq!(response.status, 404);
    assert_eq!(
        response.body.unwrap()["error"]["type"],
        "index_not_found_exception"
    );
}

#[tokio::test]
async fn missing_document_index_and_alias_misses_are_explicit() {
    let state = ephemeral_state();

    let get = call(&state, Method::GET, "/absent/_doc/1", Value::Null).await;
    assert_eq!(get.status, 404);
    assert_eq!(
        get.body.unwrap()["error"]["type"],
        "index_not_found_exception"
    );

    call(&state, Method::PUT, "/aliased", json!({})).await;
    let alias = call(&state, Method::GET, "/_alias/missing", Value::Null).await;
    assert_eq!(alias.status, 404);
    assert_eq!(
        alias.body.unwrap()["error"]["type"],
        "aliases_not_found_exception"
    );
}

#[tokio::test]
async fn browser_cross_site_write_guards_are_rejected() {
    let state = ephemeral_state();
    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_static("example.test:9200"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));

    let response = raw_call(
        &state,
        Method::PUT,
        "/guarded/_doc/1",
        headers,
        Bytes::from(r#"{"ok":true}"#),
    )
    .await;

    assert_eq!(response.status, 403);
}

#[tokio::test]
async fn opensearch_vendor_json_content_type_is_allowed_case_insensitively() {
    let state = ephemeral_state();
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("Application/Vnd.OpenSearch+Json; Compatible-With=3"),
    );

    let response = raw_call(
        &state,
        Method::PUT,
        "/vendor-content/_doc/1",
        headers,
        Bytes::from(r#"{"ok":true}"#),
    )
    .await;

    assert_eq!(response.status, 201);
}

#[tokio::test]
async fn durable_mode_locks_data_dir() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.data_dir = temp.path().to_path_buf();

    let first = AppState::new(config.clone()).unwrap();
    let second = AppState::new(config);

    assert!(second.is_err());
    drop(first);
}
