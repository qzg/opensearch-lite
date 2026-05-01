pub mod aliases;
pub mod bulk;
pub mod cat;
pub mod cluster;
pub mod documents;
pub mod indices;
pub mod search;
pub mod templates;

use std::collections::BTreeSet;

use http::Method;
use serde_json::{json, Value};

use crate::{
    agent::{
        errors::AgentError,
        tools::{apply_tool_calls, tool_catalog, AgentWriteScope, ToolExecutionError},
        validation::{
            failure_response, validate_wrapper_value, validate_write_wrapper_before_tools,
            ValidationMode,
        },
        AgentRequestContext,
    },
    api_spec::{self, Tier},
    catalog::{
        field_caps::{field_caps_response, FieldCapsRequest},
        registry,
    },
    http::request::Request,
    responses::{acknowledged, best_effort, info, logging, open_search_error, Response},
    rest_path::decode_path_param,
    search::{self as search_engine, dsl::SearchRequest},
    security,
    server::AppState,
    storage::{
        mutation_log::Mutation, Database, Store, StoreError, StoreResult, WriteOperation,
        WriteOutcome,
    },
};

const MAX_SCROLL_RETAINED_HITS: usize = 10_000;
const MAX_SCROLL_RETAINED_BYTES: usize = 32 * 1024 * 1024;

pub async fn handle_request(state: AppState, request: Request) -> Response {
    let route = api_spec::classify(&request.method, &request.path);
    if let Err(response) = security::authz::authorize(&request, &route) {
        return response;
    }
    handle_classified_request(state, request, route).await
}

pub async fn handle_classified_request(
    state: AppState,
    request: Request,
    route: api_spec::RouteMatch,
) -> Response {
    if let Some(response) = strict_guard(&state, &route) {
        return response;
    }

    match route.tier {
        Tier::Implemented => handle_implemented(state, request, route.api_name).await,
        Tier::BestEffort => handle_best_effort(state, request, route.api_name),
        Tier::Mocked => handle_mocked(request, route.api_name),
        Tier::AgentRead => handle_agent_read(state, request, route.api_name).await,
        Tier::AgentWrite => handle_agent_write(state, request, route.api_name).await,
        Tier::Unsupported | Tier::OutsideIdentity => unsupported(route.api_name),
    }
}

async fn handle_implemented(state: AppState, request: Request, api_name: &str) -> Response {
    if request.path == "/" {
        return if request.method == Method::HEAD {
            Response::empty(200)
        } else {
            info::root_info(&state.config)
        };
    }

    let parts = segments(&request.path);
    if parts.first() == Some(&"_tasks") && parts.len() == 2 {
        let task_id = decode_path_param(parts[1]);
        return handle_task_get(&state, &task_id);
    }
    if request.path == "/_reindex" {
        return handle_reindex(&state, &request).await;
    }
    if request.path == "/_search/scroll" || request.path == "/_scroll" {
        return if request.method == Method::DELETE {
            handle_clear_scroll(&state, &request, None)
        } else {
            handle_scroll(&state, &request, None)
        };
    }
    if matches!(parts.as_slice(), ["_search", "scroll", _] | ["_scroll", _]) {
        return if request.method == Method::DELETE {
            handle_clear_scroll(&state, &request, parts.last().copied())
        } else {
            handle_scroll(&state, &request, parts.last().copied())
        };
    }
    if matches!(parts.as_slice(), ["_bulk"] | [_, "_bulk"]) {
        let path_index = match parts.as_slice() {
            ["_bulk"] => None,
            [index, "_bulk"] => Some(*index),
            _ => None,
        };
        return handle_bulk(&state, &request, path_index).await;
    }
    if parts.len() == 3 && parts.get(1) == Some(&"_source") {
        let id = decode_path_param(parts[2]);
        return handle_source(&state, &request, parts[0], &id);
    }
    if matches!(parts.as_slice(), ["_search"] | [_, "_search"]) {
        return handle_search(&state, &request, parts.first().copied());
    }
    if matches!(parts.as_slice(), [_, "_delete_by_query"]) {
        return handle_delete_by_query(&state, &request, parts.first().copied()).await;
    }
    if matches!(parts.as_slice(), [_, "_update_by_query"]) {
        return handle_update_by_query(&state, &request, parts.first().copied()).await;
    }
    if matches!(parts.as_slice(), ["_count"] | [_, "_count"]) {
        return handle_count(&state, &request, parts.first().copied());
    }
    if matches!(parts.as_slice(), ["_mget"] | [_, "_mget"]) {
        return handle_mget(&state, &request, parts.first().copied());
    }
    if matches!(parts.as_slice(), ["_msearch"] | [_, "_msearch"]) {
        return handle_msearch(&state, &request, parts.first().copied());
    }
    if matches!(parts.as_slice(), ["_field_caps"] | [_, "_field_caps"]) {
        return handle_field_caps(&state, &request, parts.first().copied());
    }
    if request.path == "/_cluster/stats" {
        return handle_cluster_stats(&state);
    }
    if matches!(parts.as_slice(), ["_analyze"] | [_, "_analyze"]) {
        return handle_analyze(&request);
    }
    if parts.as_slice() == ["_validate", "query"]
        || matches!(parts.as_slice(), [_, "_validate", "query"])
    {
        return handle_validate_query(&state, &request, parts.first().copied());
    }
    if parts.first() == Some(&"_cat") && parts.get(1) == Some(&"plugins") {
        return Response::json(200, json!([]));
    }
    if parts.first() == Some(&"_cat") && parts.get(1) == Some(&"templates") {
        return Response::json(200, json!([]));
    }
    if parts.first() == Some(&"_stats") || parts.get(1) == Some(&"_stats") {
        return handle_stats(&state, &parts);
    }
    if matches!(parts.as_slice(), ["_refresh"] | [_, "_refresh"]) {
        return handle_refresh(&state, parts.first().copied());
    }
    if (parts.first() == Some(&"_mapping") && parts.get(1) == Some(&"field") && parts.len() == 3)
        || (parts.get(1) == Some(&"_mapping") && parts.get(2) == Some(&"field") && parts.len() == 4)
    {
        return handle_field_mapping(&state, &request, &parts);
    }
    if matches!(parts.as_slice(), ["_mapping"] | [_, "_mapping"]) {
        return handle_mapping(&state, &request, parts.first().copied()).await;
    }
    if matches!(parts.as_slice(), ["_settings"] | [_, "_settings"]) {
        return handle_settings(&state, &request, parts.first().copied()).await;
    }
    if parts.first() == Some(&"_index_template") {
        return handle_template(&state, &request, parts.get(1).copied()).await;
    }
    if parts.first() == Some(&"_component_template") {
        return handle_component_template(&state, &request, parts.get(1).copied()).await;
    }
    if parts.first() == Some(&"_template") {
        return handle_legacy_template(&state, &request, parts.get(1).copied()).await;
    }
    if matches!(
        parts.as_slice(),
        ["_ingest", "pipeline"] | ["_ingest", "pipeline", _]
    ) {
        return handle_registry_namespace(
            &state,
            &request,
            registry::INGEST_PIPELINE,
            parts.get(2).copied(),
            "ingest.get_pipeline",
        )
        .await;
    }
    if matches!(
        parts.as_slice(),
        ["_search", "pipeline"] | ["_search", "pipeline", _]
    ) {
        return handle_registry_namespace(
            &state,
            &request,
            registry::SEARCH_PIPELINE,
            parts.get(2).copied(),
            "search_pipeline.get",
        )
        .await;
    }
    if matches!(parts.as_slice(), ["_scripts", _] | ["_scripts", _, _]) {
        return handle_script_registry(&state, &request, parts[1]).await;
    }
    if parts.first() == Some(&"_alias")
        || parts.first() == Some(&"_aliases")
        || parts.get(1) == Some(&"_alias")
        || parts.get(1) == Some(&"_aliases")
    {
        return handle_alias(&state, &request, &parts).await;
    }
    if matches!(parts.as_slice(), [_, "_explain", _]) {
        let id = decode_path_param(parts[2]);
        return handle_explain(&state, &request, parts[0], &id);
    }
    if parts.len() >= 2 && matches!(parts[1], "_doc" | "_create" | "_update") {
        return handle_document(&state, &request, &parts).await;
    }
    if parts.len() == 1 {
        return handle_index(&state, &request, parts[0]).await;
    }

    open_search_error(
        404,
        "opensearch_lite_route_exception",
        format!("implemented route [{api_name}] did not match a handler"),
        Some("Use a documented OpenSearch REST path or check docs/supported-apis.md."),
    )
}

fn handle_best_effort(state: AppState, request: Request, api_name: &str) -> Response {
    logging::approximation(api_name, &request.path);
    let parts = segments(&request.path);
    match api_name {
        "nodes.info" => {
            let node_ip = state.config.listen.ip().to_string();
            best_effort::nodes_info(
                api_name,
                &state.config.advertised_version,
                &node_ip,
                &state.config.listen.to_string(),
                request.query_value("filter_path"),
            )
        }
        "nodes.stats" => handle_nodes_stats(&state, &request, api_name),
        _ => match request.path.as_str() {
            "/_cluster/health" => best_effort::cluster_health(api_name),
            "/_cluster/settings" => Response::json(
                200,
                json!({
                    "persistent": {},
                    "transient": {},
                    "defaults": {}
                }),
            )
            .compatibility_signal(api_name, "best_effort"),
            _ if parts.first() == Some(&"_cat") && parts.get(1) == Some(&"indices") => {
                cat_indices(&state, &request, api_name)
            }
            path if path.starts_with("/_cat/health") => Response::json(
                200,
                json!([{
                    "epoch": "0",
                    "timestamp": "00:00:00",
                    "cluster": "opensearch-lite",
                    "status": "green",
                    "node.total": "1",
                    "node.data": "1"
                }]),
            )
            .compatibility_signal(api_name, "best_effort"),
            _ => best_effort::empty_metadata(api_name),
        },
    }
}

fn handle_mocked(request: Request, api_name: &str) -> Response {
    logging::approximation(api_name, &request.path);
    match api_name {
        "cluster.allocation_explain" => Response::json(
            200,
            json!({
                "index": null,
                "shard": null,
                "primary": null,
                "current_state": "started",
                "can_remain_on_current_node": "yes",
                "can_rebalance_cluster": "no",
                "rebalance_explanation": "OpenSearch Lite runs as a single local node; shard allocation is not modeled.",
                "opensearch_lite": mocked_note(api_name)
            }),
        )
        .compatibility_signal(api_name, "mocked"),
        "cluster.put_settings" => {
            let body = match request.body_json() {
                Ok(body) => redact_secret_values(body),
                Err(error) => return parse_error(error),
            };
            if !body.is_object() {
                return parse_error("cluster settings body must be a JSON object".to_string());
            }
            let persistent = match body.get("persistent") {
                Some(value) if !value.is_object() => {
                    return parse_error(
                        "cluster settings [persistent] must be a JSON object".to_string(),
                    );
                }
                Some(value) => value.clone(),
                None => json!({}),
            };
            let transient = match body.get("transient") {
                Some(value) if !value.is_object() => {
                    return parse_error(
                        "cluster settings [transient] must be a JSON object".to_string(),
                    );
                }
                Some(value) => value.clone(),
                None => json!({}),
            };
            Response::json(
                200,
                json!({
                    "acknowledged": true,
                    "persistent": persistent,
                    "transient": transient,
                    "opensearch_lite": mocked_note(api_name)
                }),
            )
            .compatibility_signal(api_name, "mocked")
        }
        "indices.clear_cache"
        | "indices.flush"
        | "indices.forcemerge"
        | "indices.open"
        | "indices.upgrade" => Response::json(
            200,
            json!({
                "_shards": {
                    "total": 1,
                    "successful": 1,
                    "failed": 0
                },
                "acknowledged": true,
                "shards_acknowledged": true,
                "opensearch_lite": mocked_note(api_name)
            }),
        )
        .compatibility_signal(api_name, "mocked"),
        "delete_by_query_rethrottle" | "reindex_rethrottle" | "update_by_query_rethrottle" => {
            Response::json(
                200,
                json!({
                    "nodes": {},
                    "opensearch_lite": mocked_note(api_name)
                }),
            )
            .compatibility_signal(api_name, "mocked")
        }
        _ => Response::json(
            200,
            json!({
                "acknowledged": true,
                "opensearch_lite": mocked_note(api_name)
            }),
        )
        .compatibility_signal(api_name, "mocked"),
    }
}

fn mocked_note(api_name: &str) -> Value {
    json!({
        "tier": "mocked",
        "api": api_name,
        "reason": "This API is a benign local no-op in OpenSearch Lite's single-node development runtime.",
        "next_step": "If this behavior matters for your application, test against full OpenSearch locally, server-hosted OpenSearch, or cloud-hosted OpenSearch."
    })
}

async fn handle_agent_read(state: AppState, request: Request, api_name: &str) -> Response {
    let body = if request.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&request.body)
            .map(redact_secret_values)
            .unwrap_or_else(|_| {
                json!({
                    "unparsed_body_omitted": true
                })
            })
    };
    let query = redact_secret_values(query_value(&request.query));
    let catalog = match state
        .store
        .read_database(|db| agent_catalog_context(db, &request, api_name))
    {
        Ok(catalog) => catalog,
        Err(error) => return store_error(error),
    };
    let context = AgentRequestContext {
        method: request.method.as_str().to_string(),
        path: request.path.clone(),
        query,
        body,
        api_name: api_name.to_string(),
        route_tier: "agent_fallback_eligible".to_string(),
        catalog,
        tools: tool_catalog(api_name, false),
    };
    match state.agent.complete(context).await {
        Ok(response) => response,
        Err(error) => failure_response(error),
    }
}

async fn handle_agent_write(state: AppState, request: Request, api_name: &str) -> Response {
    if !state.config.agent.write_enabled_for(api_name) {
        return write_fallback_disabled(api_name);
    }
    let body = if request.body.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&request.body) {
            Ok(body) => body,
            Err(error) => return parse_error(error.to_string()),
        }
    };
    let scope = match agent_write_scope(api_name, &request, body.clone()) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let context_body = redact_secret_values(body);
    let query = redact_secret_values(query_value(&request.query));
    let catalog = match state
        .store
        .read_database(|db| agent_catalog_context(db, &request, api_name))
    {
        Ok(catalog) => catalog,
        Err(error) => return store_error(error),
    };
    let context = AgentRequestContext {
        method: request.method.as_str().to_string(),
        path: request.path.clone(),
        query,
        body: context_body,
        api_name: api_name.to_string(),
        route_tier: "agent_write_fallback_eligible".to_string(),
        catalog,
        tools: tool_catalog(api_name, true),
    };
    let wrapper = match state.agent.complete_raw(context).await {
        Ok(wrapper) => wrapper,
        Err(error) => return failure_response(error),
    };
    if let Err(error) =
        validate_write_wrapper_before_tools(&wrapper, state.config.agent.confidence_threshold)
    {
        return failure_response(error);
    }
    let tool_calls = wrapper.tool_calls.clone();
    let store = state.store.clone();
    let tool_summary =
        match tokio::task::spawn_blocking(move || apply_tool_calls(&store, &scope, &tool_calls))
            .await
        {
            Ok(Ok(summary)) => summary,
            Ok(Err(ToolExecutionError::Agent(error))) => return failure_response(error),
            Ok(Err(ToolExecutionError::Store(error))) => return store_error(error),
            Err(error) => {
                return failure_response(AgentError::new(
                    format!("agent write tool execution failed: {error}"),
                    "Retry the request or use a deterministic implemented API.",
                ))
            }
        };
    validate_wrapper_value(
        wrapper,
        state.config.agent.confidence_threshold,
        ValidationMode::Write {
            commit_performed: tool_summary.committed,
        },
    )
    .unwrap_or_else(failure_response)
}

fn agent_write_scope(
    api_name: &str,
    request: &Request,
    body: Value,
) -> Result<AgentWriteScope, Response> {
    match api_name {
        "indices.put_template" => {
            let parts = segments(&request.path);
            match parts.as_slice() {
                ["_template", name] => Ok(AgentWriteScope::legacy_template(*name, body)),
                _ => Err(unsupported(api_name)),
            }
        }
        _ => Err(unsupported(api_name)),
    }
}

async fn handle_index(state: &AppState, request: &Request, index: &str) -> Response {
    match request.method {
        Method::PUT => match request.body_json() {
            Ok(body) => {
                let index = index.to_string();
                let index_for_store = index.clone();
                match run_store(state.store.clone(), move |store| {
                    store.create_index(&index_for_store, body)
                })
                .await
                {
                    Ok(()) => Response::json(
                        200,
                        json!({
                            "acknowledged": true,
                            "shards_acknowledged": true,
                            "index": index
                        }),
                    ),
                    Err(error) => store_error(error),
                }
            }
            Err(error) => parse_error(error),
        },
        Method::GET => {
            let db = state.store.database();
            let Some(index_name) = db.resolve_index(index) else {
                return store_error(StoreError::new(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                ));
            };
            let Some(meta) = db.indexes.get(&index_name) else {
                return store_error(StoreError::new(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                ));
            };
            Response::json(
                200,
                json!({
                    index_name: {
                        "aliases": meta.aliases.iter().map(|alias| (alias.clone(), json!({}))).collect::<serde_json::Map<_, _>>(),
                        "mappings": meta.mappings,
                        "settings": meta.settings
                    }
                }),
            )
        }
        Method::HEAD => {
            if state.store.resolve_index(index).is_some() {
                Response::empty(200)
            } else {
                Response::empty(404)
            }
        }
        Method::DELETE => {
            let index = index.to_string();
            match run_store(state.store.clone(), move |store| store.delete_index(&index)).await {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported("indices"),
    }
}

async fn handle_template(state: &AppState, request: &Request, name: Option<&str>) -> Response {
    match request.method {
        Method::PUT => {
            let Some(name) = name else {
                return parse_error("template name is required".to_string());
            };
            if request.query_value("create") == Some("true")
                && state.store.database().templates.contains_key(name)
            {
                return store_error(StoreError::new(
                    400,
                    "resource_already_exists_exception",
                    format!("index template [{name}] already exists"),
                ));
            }
            match request.body_json() {
                Ok(body) => {
                    let name = name.to_string();
                    match run_store(state.store.clone(), move |store| {
                        store.put_template(&name, body)
                    })
                    .await
                    {
                        Ok(()) => acknowledged(true),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        Method::GET => {
            let db = state.store.database();
            if let Some(name) = name {
                if !db.templates.contains_key(name) {
                    return store_error(StoreError::new(
                        404,
                        "index_template_missing_exception",
                        format!("index template [{name}] missing"),
                    ));
                }
            }
            let templates = db
                .templates
                .iter()
                .filter(|(template_name, _)| {
                    name.map(|name| name == template_name.as_str())
                        .unwrap_or(true)
                })
                .map(|(name, template)| {
                    json!({
                        "name": name,
                        "index_template": normalize_index_template_response(&template.raw)
                    })
                })
                .collect::<Vec<_>>();
            Response::json(200, json!({ "index_templates": templates }))
        }
        Method::HEAD => {
            let Some(name) = name else {
                return Response::empty(400);
            };
            let db = state.store.database();
            if db.templates.contains_key(name) {
                Response::empty(200)
            } else {
                Response::empty(404)
            }
        }
        Method::DELETE => {
            let Some(name) = name else {
                return parse_error("template name is required".to_string());
            };
            let name = name.to_string();
            match run_store(state.store.clone(), move |store| {
                store.commit(Mutation::DeleteTemplate { name })
            })
            .await
            {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported("indices.put_index_template"),
    }
}

async fn handle_component_template(
    state: &AppState,
    request: &Request,
    name: Option<&str>,
) -> Response {
    match request.method {
        Method::PUT => {
            let Some(name) = name else {
                return parse_error("component template name is required".to_string());
            };
            match request.body_json() {
                Ok(body) => {
                    let name = name.to_string();
                    match run_store(state.store.clone(), move |store| {
                        store.put_registry_object(registry::COMPONENT_TEMPLATE, &name, body)
                    })
                    .await
                    {
                        Ok(()) => acknowledged(true),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        Method::GET => {
            let db = state.store.database();
            registry::get_component_templates(&db, name)
        }
        Method::HEAD => {
            let Some(name) = name else {
                return Response::empty(400);
            };
            if registry_exists(&state.store.database(), registry::COMPONENT_TEMPLATE, name) {
                Response::empty(200)
            } else {
                Response::empty(404)
            }
        }
        Method::DELETE => {
            let Some(name) = name else {
                return parse_error("component template name is required".to_string());
            };
            let name = name.to_string();
            match run_store(state.store.clone(), move |store| {
                store.delete_registry_object(registry::COMPONENT_TEMPLATE, &name)
            })
            .await
            {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported("cluster.put_component_template"),
    }
}

fn normalize_index_template_response(raw: &Value) -> Value {
    let mut raw = raw.clone();
    if let Some(pattern) = raw.get("index_patterns").and_then(Value::as_str) {
        raw["index_patterns"] = json!([pattern]);
    }
    if let Some(settings) = raw
        .get_mut("template")
        .and_then(|template| template.get_mut("settings"))
    {
        normalize_settings_response(settings);
    }
    if let Some(aliases) = raw
        .get_mut("template")
        .and_then(|template| template.get_mut("aliases"))
        .and_then(Value::as_object_mut)
    {
        for metadata in aliases.values_mut() {
            *metadata = normalize_alias_metadata(metadata.take());
        }
    }
    raw
}

fn normalize_settings_response(settings: &mut Value) {
    let Some(object) = settings.as_object_mut() else {
        return;
    };
    if object.contains_key("index") {
        if let Some(index) = object.get_mut("index") {
            stringify_object_values(index);
        }
        return;
    }
    let mut index = serde_json::Map::new();
    for (key, value) in std::mem::take(object) {
        index.insert(key, stringify_value(value));
    }
    object.insert("index".to_string(), Value::Object(index));
}

fn stringify_object_values(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for value in object.values_mut() {
            *value = stringify_value(value.take());
        }
    }
}

fn stringify_value(value: Value) -> Value {
    match value {
        Value::Number(number) => Value::String(number.to_string()),
        Value::Bool(value) => Value::String(value.to_string()),
        value => value,
    }
}

fn analyze_text(text: &str, analyzer: &str) -> Vec<(String, usize, usize)> {
    if analyzer == "keyword" {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![(text.to_string(), 0, text.len())]
        };
    }
    let mut tokens = Vec::new();
    let mut current_start = None;
    for (offset, ch) in text.char_indices() {
        let boundary = if analyzer == "whitespace" {
            ch.is_whitespace()
        } else {
            !ch.is_alphanumeric()
        };
        if boundary {
            if let Some(start) = current_start.take() {
                tokens.push((text[start..offset].to_ascii_lowercase(), start, offset));
            }
        } else if current_start.is_none() {
            current_start = Some(offset);
        }
    }
    if let Some(start) = current_start {
        tokens.push((text[start..].to_ascii_lowercase(), start, text.len()));
    }
    tokens
}

async fn handle_mapping(state: &AppState, request: &Request, first: Option<&str>) -> Response {
    let path_index = first.filter(|index| *index != "_mapping");
    match request.method {
        Method::GET => catalog_subset(state, path_index, "mappings"),
        Method::PUT => {
            let Some(index) = path_index else {
                return parse_error("put mapping requires an index path".to_string());
            };
            match request.body_json() {
                Ok(body) => {
                    let index = index.to_string();
                    match run_store(state.store.clone(), move |store| {
                        store.put_mapping(&index, body)
                    })
                    .await
                    {
                        Ok(()) => acknowledged(true),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        _ => unsupported("indices.put_mapping"),
    }
}

async fn handle_settings(state: &AppState, request: &Request, first: Option<&str>) -> Response {
    let path_index = first.filter(|index| *index != "_settings");
    match request.method {
        Method::GET => catalog_subset(state, path_index, "settings"),
        Method::PUT => {
            let Some(index) = path_index else {
                return parse_error("put settings requires an index path".to_string());
            };
            match request.body_json() {
                Ok(body) => {
                    let index = index.to_string();
                    let settings = body.get("settings").cloned().unwrap_or(body);
                    match run_store(state.store.clone(), move |store| {
                        store.put_settings(&index, settings)
                    })
                    .await
                    {
                        Ok(()) => acknowledged(true),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        _ => unsupported("indices.put_settings"),
    }
}

fn handle_field_caps(state: &AppState, request: &Request, first: Option<&str>) -> Response {
    let path_index = first.filter(|index| *index != "_field_caps");
    if !request.body.is_empty() {
        let body = match request.body_json() {
            Ok(body) => body,
            Err(error) => return parse_error(error),
        };
        if body
            .as_object()
            .map(|object| !object.is_empty())
            .unwrap_or(true)
        {
            return open_search_error(
                400,
                "x_content_parse_exception",
                "field_caps request body is not supported by OpenSearch Lite yet",
                Some("Move field selection to the fields query parameter, or retry without index_filter."),
            );
        }
    }
    let fields = match comma_query_values(request.query_value("fields")) {
        Some(fields) if !fields.is_empty() => fields,
        _ => return parse_error("field_caps requires non-empty [fields]".to_string()),
    };
    let indices = path_indices(path_index, "_field_caps");
    let field_caps_request = FieldCapsRequest {
        indices,
        fields,
        ignore_unavailable: bool_query(request.query_value("ignore_unavailable")),
        allow_no_indices: bool_query(request.query_value("allow_no_indices")),
    };
    match state
        .store
        .read_database(|db| field_caps_response(db, field_caps_request))
    {
        Ok(Ok(body)) => Response::json(200, body),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

fn handle_cluster_stats(state: &AppState) -> Response {
    match state.store.read_database(cluster_stats_response) {
        Ok(body) => Response::json(200, body),
        Err(error) => store_error(error),
    }
}

fn handle_nodes_stats(state: &AppState, request: &Request, api_name: &str) -> Response {
    let stats = state.store.read_database(|db| {
        let docs = db.document_count();
        let deleted = db
            .indexes
            .values()
            .map(|index| index.tombstones.len())
            .sum::<usize>();
        let store_bytes = db
            .indexes
            .values()
            .map(|index| index.store_size_bytes)
            .sum::<usize>();
        (docs, deleted, store_bytes)
    });
    match stats {
        Ok((docs, deleted, store_bytes)) => {
            let node_ip = state.config.listen.ip().to_string();
            let publish_address = state.config.listen.to_string();
            best_effort::nodes_stats(
                api_name,
                best_effort::NodesStatsMetadata {
                    advertised_version: &state.config.advertised_version,
                    node_ip: &node_ip,
                    publish_address: &publish_address,
                    docs,
                    deleted,
                    store_bytes,
                    filter_path: request.query_value("filter_path"),
                },
            )
        }
        Err(error) => store_error(error),
    }
}

fn handle_validate_query(state: &AppState, request: &Request, first: Option<&str>) -> Response {
    let body = if request.body.is_empty() {
        json!({ "query": { "match_all": {} } })
    } else {
        match request.body_json() {
            Ok(body) => body,
            Err(error) => return parse_error(error),
        }
    };
    let query = body
        .get("query")
        .cloned()
        .unwrap_or_else(|| json!({ "match_all": {} }));
    if let Err(response) = validate_query_only(state, request.body.len(), &query) {
        return response;
    }
    let indices = path_indices(first, "_validate");
    let shard_count = match state.store.read_database(|db| {
        let names = resolve_index_patterns(db, &indices)?;
        Ok(names
            .iter()
            .filter_map(|name| db.indexes.get(name))
            .map(index_total_shards)
            .sum::<u64>()
            .max(1))
    }) {
        Ok(Ok(shards)) => shards,
        Ok(Err(error)) | Err(error) => return store_error(error),
    };
    let mut body = json!({
        "_shards": {
            "total": shard_count,
            "successful": shard_count,
            "failed": 0
        },
        "valid": true
    });
    if request.query_value("explain") == Some("true") {
        body["explanations"] = json!([{
            "index": indices.first().cloned().unwrap_or_else(|| "_all".to_string()),
            "valid": true,
            "explanation": "query is valid for OpenSearch Lite's local evaluator"
        }]);
    }
    Response::json(200, body)
}

fn handle_analyze(request: &Request) -> Response {
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    let analyzer = body
        .get("analyzer")
        .or_else(|| body.get("tokenizer"))
        .and_then(Value::as_str)
        .unwrap_or("standard");
    if !matches!(
        analyzer,
        "standard" | "default" | "simple" | "whitespace" | "keyword"
    ) {
        return open_search_error(
            400,
            "illegal_argument_exception",
            format!("analyzer [{analyzer}] is not supported by OpenSearch Lite"),
            Some("Use standard/default/simple/whitespace/keyword for local analysis, or test analyzer-specific behavior against full OpenSearch."),
        );
    }
    let texts = match body.get("text") {
        Some(Value::String(text)) => vec![text.as_str()],
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        _ => {
            return open_search_error(
                400,
                "action_request_validation_exception",
                "analyze requires text",
                Some("Provide a string or array of strings in the text field."),
            );
        }
    };
    let mut tokens = Vec::new();
    let mut position = 0usize;
    for text in texts {
        for (token, start_offset, end_offset) in analyze_text(text, analyzer) {
            tokens.push(json!({
                "token": token,
                "start_offset": start_offset,
                "end_offset": end_offset,
                "type": "word",
                "position": position
            }));
            position += 1;
        }
    }
    Response::json(200, json!({ "tokens": tokens }))
}

fn handle_explain(state: &AppState, request: &Request, index: &str, id: &str) -> Response {
    let body = if request.body.is_empty() {
        json!({ "query": { "match_all": {} } })
    } else {
        match request.body_json() {
            Ok(body) => body,
            Err(error) => return parse_error(error),
        }
    };
    let query = body
        .get("query")
        .cloned()
        .unwrap_or_else(|| json!({ "match_all": {} }));
    if let Err(response) = validate_query_only(state, request.body.len(), &query) {
        return response;
    }
    let result = state.store.read_database(|db| {
        let index_name = db.resolve_index(index).ok_or_else(|| {
            StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{index}]"),
            )
        })?;
        let document = db
            .indexes
            .get(&index_name)
            .and_then(|index| index.documents.get(id))
            .ok_or_else(|| {
                StoreError::new(
                    404,
                    "document_missing_exception",
                    format!("document [{id}] missing"),
                )
            })?;
        search_engine::evaluator::document_matches(document, &query)
            .map_err(|error| StoreError::new(400, "x_content_parse_exception", error))
            .map(|matched| (index_name, matched))
    });
    match result {
        Ok(Ok((index_name, matched))) => Response::json(
            200,
            json!({
                "_index": index_name,
                "_id": id,
                "matched": matched,
                "explanation": {
                    "value": if matched { 1.0 } else { 0.0 },
                    "description": "OpenSearch Lite local evaluator match result",
                    "details": []
                }
            }),
        ),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

async fn handle_legacy_template(
    state: &AppState,
    request: &Request,
    name: Option<&str>,
) -> Response {
    match request.method {
        Method::PUT => {
            let Some(name) = name else {
                return parse_error("template name is required".to_string());
            };
            match request.body_json() {
                Ok(body) => {
                    let name = name.to_string();
                    match run_store(state.store.clone(), move |store| {
                        store.put_registry_object(registry::LEGACY_TEMPLATE, &name, body)
                    })
                    .await
                    {
                        Ok(()) => acknowledged(true),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        Method::GET => {
            let db = state.store.database();
            registry::get_named_object(
                &db,
                registry::LEGACY_TEMPLATE,
                name,
                "index_template_missing_exception",
                "legacy template",
            )
        }
        Method::HEAD => {
            let Some(name) = name else {
                return Response::empty(400);
            };
            if registry_exists(&state.store.database(), registry::LEGACY_TEMPLATE, name) {
                Response::empty(200)
            } else {
                Response::empty(404)
            }
        }
        Method::DELETE => {
            let Some(name) = name else {
                return parse_error("template name is required".to_string());
            };
            if !registry_exists(&state.store.database(), registry::LEGACY_TEMPLATE, name) {
                return store_error(StoreError::new(
                    404,
                    "index_template_missing_exception",
                    format!("index template [{name}] missing"),
                ));
            }
            let name = name.to_string();
            match run_store(state.store.clone(), move |store| {
                store.delete_registry_object(registry::LEGACY_TEMPLATE, &name)
            })
            .await
            {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported("indices.delete_template"),
    }
}

async fn handle_registry_namespace(
    state: &AppState,
    request: &Request,
    namespace: &'static str,
    name: Option<&str>,
    read_api_name: &'static str,
) -> Response {
    match request.method {
        Method::PUT => {
            let Some(name) = name else {
                return parse_error("registry object name is required".to_string());
            };
            match request.body_json() {
                Ok(body) => {
                    let name = name.to_string();
                    match run_store(state.store.clone(), move |store| {
                        store.put_registry_object(namespace, &name, body)
                    })
                    .await
                    {
                        Ok(()) => acknowledged(true),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        Method::GET => {
            let db = state.store.database();
            let (missing_type, label) = match namespace {
                registry::INGEST_PIPELINE => ("resource_not_found_exception", "ingest pipeline"),
                registry::SEARCH_PIPELINE => ("resource_not_found_exception", "search pipeline"),
                _ => ("resource_not_found_exception", "registry object"),
            };
            registry::get_named_object(&db, namespace, name, missing_type, label)
        }
        Method::DELETE => {
            let Some(name) = name else {
                return parse_error("registry object name is required".to_string());
            };
            let name = name.to_string();
            match run_store(state.store.clone(), move |store| {
                store.delete_registry_object(namespace, &name)
            })
            .await
            {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported(read_api_name),
    }
}

async fn handle_script_registry(state: &AppState, request: &Request, name: &str) -> Response {
    match request.method {
        Method::PUT | Method::POST => match request.body_json() {
            Ok(body) => {
                let name = name.to_string();
                match run_store(state.store.clone(), move |store| {
                    store.put_registry_object(registry::SCRIPT, &name, body)
                })
                .await
                {
                    Ok(()) => acknowledged(true),
                    Err(error) => store_error(error),
                }
            }
            Err(error) => parse_error(error),
        },
        Method::GET => {
            let db = state.store.database();
            registry::get_script(&db, name)
        }
        Method::DELETE => {
            let name = name.to_string();
            match run_store(state.store.clone(), move |store| {
                store.delete_registry_object(registry::SCRIPT, &name)
            })
            .await
            {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported("put_script"),
    }
}

fn registry_exists(db: &Database, namespace: &str, name: &str) -> bool {
    db.registries
        .get(namespace)
        .map(|registry| registry.contains_key(name))
        .unwrap_or(false)
}

fn catalog_subset(state: &AppState, path_index: Option<&str>, key: &str) -> Response {
    let db = state.store.database();
    let mut output = serde_json::Map::new();
    let requested = path_index
        .map(|index| index.split(',').collect::<Vec<_>>())
        .unwrap_or_else(|| db.indexes.keys().map(String::as_str).collect::<Vec<_>>());

    for requested_name in requested {
        let Some(index_name) = db.resolve_index(requested_name) else {
            return store_error(StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{requested_name}]"),
            ));
        };
        let Some(index) = db.indexes.get(&index_name) else {
            continue;
        };
        output.insert(
            index_name,
            json!({
                key: match key {
                    "mappings" => index.mappings.clone(),
                    "settings" => index.settings.clone(),
                    _ => json!({})
                }
            }),
        );
    }
    Response::json(200, Value::Object(output))
}

async fn handle_alias(state: &AppState, request: &Request, parts: &[&str]) -> Response {
    match (request.method.clone(), parts) {
        (Method::PUT, [index, "_alias", alias]) => match request.body_json() {
            Ok(body) => {
                let index = body
                    .get("index")
                    .and_then(json_scalar_string)
                    .unwrap_or_else(|| index.to_string());
                let alias = body
                    .get("alias")
                    .and_then(json_scalar_string)
                    .unwrap_or_else(|| alias.to_string());
                let body = normalize_alias_metadata(body);
                match run_store(state.store.clone(), move |store| {
                    store.put_alias(&index, &alias, body)
                })
                .await
                {
                    Ok(()) => acknowledged(true),
                    Err(error) => store_error(error),
                }
            }
            Err(error) => parse_error(error),
        },
        (Method::POST, ["_alias"]) | (Method::POST, ["_aliases"]) => {
            handle_alias_actions(state, request).await
        }
        (Method::HEAD, ["_alias", alias]) => alias_exists_response(state, None, alias),
        (Method::HEAD, [index, "_alias", alias]) => {
            alias_exists_response(state, Some(index), alias)
        }
        (Method::HEAD, [index, "_aliases", alias]) => {
            alias_exists_response(state, Some(index), alias)
        }
        (Method::GET, ["_alias"]) => alias_response(state, None, None),
        (Method::GET, ["_alias", alias]) => alias_response(state, None, Some(alias)),
        (Method::GET, ["_aliases"]) => alias_response(state, None, None),
        (Method::GET, [index, "_alias", alias]) => alias_response(state, Some(index), Some(alias)),
        (Method::GET, [index, "_aliases", alias]) => {
            alias_response(state, Some(index), Some(alias))
        }
        (Method::GET, [index, "_alias"]) => alias_response(state, Some(index), None),
        (Method::GET, [index, "_aliases"]) => alias_response(state, Some(index), None),
        (Method::DELETE, [index, "_alias", alias]) => {
            let index = index.to_string();
            let alias = alias.to_string();
            match run_store(state.store.clone(), move |store| {
                store.delete_alias(&index, &alias)
            })
            .await
            {
                Ok(()) => acknowledged(true),
                Err(error) => store_error(error),
            }
        }
        _ => unsupported("indices.put_alias"),
    }
}

async fn handle_alias_actions(state: &AppState, request: &Request) -> Response {
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    let Some(actions) = body.get("actions").and_then(Value::as_array).cloned() else {
        return parse_error("alias actions must be an array".to_string());
    };
    let mut delete_index_names = BTreeSet::new();
    let mut mutations = Vec::new();
    for action in actions {
        let Some(action) = action.as_object() else {
            return parse_error("alias action must be an object".to_string());
        };
        if action.len() != 1 {
            return parse_error(
                "alias action must contain exactly one add, remove, or remove_index operation"
                    .to_string(),
            );
        }
        let Some((kind, meta)) = action.iter().next() else {
            return parse_error(
                "alias action must contain add, remove, or remove_index".to_string(),
            );
        };
        let Some(meta) = meta.as_object() else {
            return parse_error("alias action metadata must be an object".to_string());
        };
        let indices = action_values(meta.get("index"), meta.get("indices"));
        let aliases = action_values(meta.get("alias"), meta.get("aliases"));
        match kind.as_str() {
            "add" => {
                if indices.is_empty() || aliases.is_empty() {
                    return parse_error("alias add action requires index and alias".to_string());
                }
                let raw = normalize_alias_metadata(Value::Object(meta.clone()));
                for index in &indices {
                    for alias in &aliases {
                        mutations.push(Mutation::PutAlias {
                            index: index.clone(),
                            alias: alias.clone(),
                            raw: raw.clone(),
                        });
                    }
                }
            }
            "remove" => {
                if indices.is_empty() || aliases.is_empty() {
                    return parse_error("alias remove action requires index and alias".to_string());
                }
                for index in &indices {
                    for alias in &aliases {
                        mutations.push(Mutation::DeleteAlias {
                            index: index.clone(),
                            alias: alias.clone(),
                        });
                    }
                }
            }
            "remove_index" => {
                if indices.is_empty() {
                    return parse_error("alias remove_index action requires index".to_string());
                }
                for index in &indices {
                    delete_index_names.insert(index.clone());
                    mutations.push(Mutation::DeleteIndex {
                        name: index.clone(),
                    });
                }
            }
            other => {
                return store_error(StoreError::new(
                    400,
                    "illegal_argument_exception",
                    format!("unsupported alias action [{other}]"),
                ));
            }
        }
    }
    let mutations = order_alias_mutations(mutations, &delete_index_names);
    if !mutations.is_empty() {
        match run_store(state.store.clone(), move |store| {
            store.commit_mutations(mutations)
        })
        .await
        {
            Ok(()) => {}
            Err(error) => return store_error(error),
        }
    }
    acknowledged(true)
}

fn order_alias_mutations(
    mutations: Vec<Mutation>,
    delete_index_names: &BTreeSet<String>,
) -> Vec<Mutation> {
    let mut emitted_deletes = BTreeSet::new();
    let mut ordered = Vec::new();
    for mutation in mutations {
        match &mutation {
            Mutation::PutAlias { alias, .. } if delete_index_names.contains(alias) => {
                if emitted_deletes.insert(alias.clone()) {
                    ordered.push(Mutation::DeleteIndex {
                        name: alias.clone(),
                    });
                }
                ordered.push(mutation);
            }
            Mutation::DeleteIndex { name } => {
                if emitted_deletes.insert(name.clone()) {
                    ordered.push(mutation);
                }
            }
            _ => ordered.push(mutation),
        }
    }
    ordered
}

fn action_values(one: Option<&Value>, many: Option<&Value>) -> Vec<String> {
    one.and_then(Value::as_str)
        .map(|value| vec![value.to_string()])
        .or_else(|| {
            many.and_then(Value::as_array).map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default()
}

fn alias_response(state: &AppState, index: Option<&str>, alias: Option<&str>) -> Response {
    let db = state.store.database();
    let requested_index = match index {
        Some(index) => {
            let Some(index_name) = db.resolve_index(index) else {
                return store_error(StoreError::new(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                ));
            };
            Some(index_name)
        }
        None => None,
    };
    let mut output = serde_json::Map::new();
    if alias.is_none() {
        for index_name in alias_response_index_names(&db, requested_index.as_deref()) {
            output_alias_index_entry(&mut output, &index_name);
        }
    }
    for (alias_name, meta) in &db.aliases {
        if alias.map(|alias| alias != alias_name).unwrap_or(false) {
            continue;
        }
        if requested_index
            .as_ref()
            .map(|index| index != &meta.index)
            .unwrap_or(false)
        {
            continue;
        }
        let entry = output_alias_index_entry(&mut output, &meta.index);
        entry["aliases"][alias_name] = meta.raw.clone();
    }
    if output.is_empty() && alias.is_some() {
        return store_error(StoreError::new(
            404,
            "aliases_not_found_exception",
            format!("alias [{}] missing", alias.unwrap_or_default()),
        ));
    }
    Response::json(200, Value::Object(output))
}

fn alias_response_index_names(db: &Database, requested_index: Option<&str>) -> Vec<String> {
    match requested_index {
        Some(index) => vec![index.to_string()],
        None => db.indexes.keys().cloned().collect(),
    }
}

fn output_alias_index_entry<'a>(
    output: &'a mut serde_json::Map<String, Value>,
    index: &str,
) -> &'a mut Value {
    output
        .entry(index.to_string())
        .or_insert_with(|| json!({ "aliases": {} }))
}

fn normalize_alias_metadata(raw: Value) -> Value {
    let Value::Object(mut object) = raw else {
        return json!({});
    };
    object.remove("index");
    object.remove("indices");
    object.remove("alias");
    object.remove("aliases");
    if let Some(routing) = object.remove("routing") {
        object
            .entry("index_routing".to_string())
            .or_insert_with(|| routing.clone());
        object
            .entry("search_routing".to_string())
            .or_insert(routing);
    }
    Value::Object(object)
}

fn alias_exists_response(state: &AppState, index: Option<&str>, alias: &str) -> Response {
    let db = state.store.database();
    if let Some(index) = index {
        let Some(index_name) = db.resolve_index(index) else {
            return Response::empty(404);
        };
        if db
            .aliases
            .get(alias)
            .map(|metadata| metadata.index == index_name)
            .unwrap_or(false)
        {
            Response::empty(200)
        } else {
            Response::empty(404)
        }
    } else if db.aliases.contains_key(alias) {
        Response::empty(200)
    } else {
        Response::empty(404)
    }
}

async fn handle_document(state: &AppState, request: &Request, parts: &[&str]) -> Response {
    let index = parts[0];
    let action = parts[1];
    let decoded_id = parts.get(2).map(|id| decode_path_param(id));
    let id = decoded_id.as_deref();
    let valid_shape = match action {
        "_doc" => (parts.len() == 2 && request.method == Method::POST) || parts.len() == 3,
        "_create" => parts.len() == 3,
        "_update" => parts.len() == 3,
        _ => false,
    };
    if !valid_shape {
        return unsupported("document");
    }
    match (request.method.clone(), action) {
        (Method::PUT | Method::POST, "_doc") => {
            let id = id
                .map(ToString::to_string)
                .unwrap_or_else(crate::storage::Store::generated_id);
            let created = state.store.get_document(index, &id).is_none();
            match request.body_json() {
                Ok(source) => {
                    let index = index.to_string();
                    let id_for_store = id.clone();
                    let index_for_store = index.clone();
                    match run_store(state.store.clone(), move |store| {
                        store.index_document(&index_for_store, id_for_store, source)
                    })
                    .await
                    {
                        Ok(doc) => doc_write_response(
                            &index,
                            &id,
                            &doc,
                            if created { 201 } else { 200 },
                            if created { "created" } else { "updated" },
                        ),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        (Method::PUT | Method::POST, "_create") => {
            let Some(id) = id else {
                return parse_error("_create requires an explicit document id".to_string());
            };
            match request.body_json() {
                Ok(source) => {
                    let index = index.to_string();
                    let id = id.to_string();
                    let id_for_store = id.clone();
                    let index_for_store = index.clone();
                    match run_store(state.store.clone(), move |store| {
                        store.create_document(&index_for_store, id_for_store, source)
                    })
                    .await
                    {
                        Ok(doc) => doc_write_response(&index, &id, &doc, 201, "created"),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        (Method::GET, "_doc") => {
            let Some(id) = id else {
                return parse_error("document id is required".to_string());
            };
            if state.store.resolve_index(index).is_none() {
                return store_error(StoreError::new(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                ));
            }
            match state.store.get_document(index, id) {
                Some(doc) => Response::json(
                    200,
                    json!({
                        "_index": index,
                        "_id": id,
                        "_version": doc.version,
                        "_seq_no": doc.seq_no,
                        "_primary_term": doc.primary_term,
                        "found": true,
                        "_source": doc.source
                    }),
                ),
                None => Response::json(404, json!({ "_index": index, "_id": id, "found": false })),
            }
        }
        (Method::HEAD, "_doc") => {
            let Some(id) = id else {
                return Response::empty(400);
            };
            if state.store.resolve_index(index).is_none() {
                Response::empty(404)
            } else if state.store.get_document(index, id).is_some() {
                Response::empty(200)
            } else {
                Response::empty(404)
            }
        }
        (Method::DELETE, "_doc") => {
            let Some(id) = id else {
                return parse_error("document id is required".to_string());
            };
            let deleted_version = state
                .store
                .get_document(index, id)
                .map(|document| document.version.saturating_add(1));
            let index = index.to_string();
            let id = id.to_string();
            let index_for_store = index.clone();
            let id_for_store = id.clone();
            match run_store(state.store.clone(), move |store| {
                store.delete_document(&index_for_store, &id_for_store)
            })
            .await
            {
                Ok(found) => {
                    let mut body = json!({
                        "_index": index,
                        "_id": id,
                        "result": if found { "deleted" } else { "not_found" },
                        "_shards": { "total": 1, "successful": 1, "failed": 0 }
                    });
                    if found {
                        if let Some(version) = deleted_version {
                            body["_version"] = json!(version);
                        }
                    }
                    Response::json(if found { 200 } else { 404 }, body)
                }
                Err(error) => store_error(error),
            }
        }
        (Method::POST, "_update") => {
            let Some(id) = id else {
                return parse_error("document id is required".to_string());
            };
            match request.body_json() {
                Ok(body) => {
                    let update = match parse_update_body(&body) {
                        Ok(update) => update,
                        Err(error) if error.status == 501 => return unsupported("update.script"),
                        Err(error) => return store_error(error),
                    };
                    let source_filter =
                        source_filter_from_query(&request.query).or(update.source_filter);
                    let index = index.to_string();
                    let id = id.to_string();
                    let index_for_store = index.clone();
                    let id_for_store = id.clone();
                    match run_store(state.store.clone(), move |store| {
                        store.update_document(
                            &index_for_store,
                            &id_for_store,
                            update.doc,
                            update.doc_as_upsert,
                            update.upsert,
                        )
                    })
                    .await
                    {
                        Ok(doc) => {
                            let result = if doc.version == 1 {
                                "created"
                            } else {
                                "updated"
                            };
                            let mut response = doc_write_body(&index, &id, &doc, result);
                            if let Some(source_filter) = source_filter.as_ref() {
                                if source_filter != &Value::Bool(false) {
                                    response["get"] = json!({
                                        "_source": search_engine::evaluator::filter_source(
                                            &doc.source,
                                            Some(source_filter),
                                        )
                                    });
                                }
                            }
                            Response::json(if result == "created" { 201 } else { 200 }, response)
                        }
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        _ => unsupported("document"),
    }
}

struct ParsedUpdateBody {
    doc: Value,
    doc_as_upsert: bool,
    upsert: Option<Value>,
    source_filter: Option<Value>,
}

fn parse_update_body(body: &Value) -> StoreResult<ParsedUpdateBody> {
    let Some(object) = body.as_object() else {
        return Err(update_parse_error("update body must be a JSON object"));
    };
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "doc" | "upsert" | "doc_as_upsert" | "_source" | "detect_noop" | "script"
        ) {
            return Err(update_parse_error(format!(
                "unknown update body field [{key}]"
            )));
        }
    }
    if object.contains_key("script") {
        return Err(StoreError::new(
            501,
            "opensearch_lite_unsupported_api_exception",
            "OpenSearch Lite does not implement [update.script] yet",
        ));
    }
    let doc = object
        .get("doc")
        .cloned()
        .ok_or_else(|| update_parse_error("update body must include a [doc] object"))?;
    if !doc.is_object() {
        return Err(update_parse_error("update [doc] must be a JSON object"));
    }
    let upsert = object.get("upsert").cloned();
    if let Some(upsert) = upsert.as_ref() {
        if !upsert.is_object() {
            return Err(update_parse_error("update [upsert] must be a JSON object"));
        }
    }
    let doc_as_upsert = match object.get("doc_as_upsert") {
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            return Err(update_parse_error(
                "update [doc_as_upsert] must be a boolean",
            ));
        }
        None => false,
    };
    if let Some(detect_noop) = object.get("detect_noop") {
        if !detect_noop.is_boolean() {
            return Err(update_parse_error("update [detect_noop] must be a boolean"));
        }
    }

    Ok(ParsedUpdateBody {
        doc,
        doc_as_upsert,
        upsert,
        source_filter: object.get("_source").cloned(),
    })
}

fn update_parse_error(reason: impl Into<String>) -> StoreError {
    StoreError::new(400, "parse_exception", reason)
}

enum SourceLookup {
    MissingIndex,
    MissingSource,
    Found(Value),
}

fn handle_source(state: &AppState, request: &Request, index: &str, id: &str) -> Response {
    let lookup = state.store.read_database(|db| {
        let Some(index_name) = db.resolve_index(index) else {
            return SourceLookup::MissingIndex;
        };
        let Some(index_meta) = db.indexes.get(&index_name) else {
            return SourceLookup::MissingIndex;
        };
        if source_disabled(&index_meta.mappings) {
            return SourceLookup::MissingSource;
        }
        match index_meta.documents.get(id) {
            Some(doc) => SourceLookup::Found(doc.source.clone()),
            None => SourceLookup::MissingSource,
        }
    });
    let lookup = match lookup {
        Ok(lookup) => lookup,
        Err(error) => return store_error(error),
    };
    match lookup {
        SourceLookup::MissingIndex => {
            if request.method == Method::HEAD {
                Response::empty(404)
            } else {
                store_error(StoreError::new(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                ))
            }
        }
        SourceLookup::MissingSource => {
            if request.method == Method::HEAD {
                Response::empty(404)
            } else {
                store_error(StoreError::new(
                    404,
                    "document_missing_exception",
                    format!("document [{id}] missing"),
                ))
            }
        }
        SourceLookup::Found(source) => {
            if request.method == Method::HEAD {
                Response::empty(200)
            } else {
                let source = search_engine::evaluator::filter_source(
                    &source,
                    source_filter_from_query(&request.query).as_ref(),
                );
                Response::json(200, source)
            }
        }
    }
}

async fn handle_bulk(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
    let text = match std::str::from_utf8(&request.body) {
        Ok(text) => text,
        Err(error) => return parse_error(format!("bulk body must be UTF-8 NDJSON: {error}")),
    };
    let mut lines = text.lines();
    let mut plans = Vec::new();
    let mut action_count = 0usize;

    while let Some(action_line) = lines.next() {
        if action_line.trim().is_empty() {
            plans.push(BulkPlan::Immediate(
                json!({"index": bulk_parse_error("bulk action line must not be empty")}),
            ));
            continue;
        }
        action_count += 1;
        if action_count > state.config.max_bulk_actions {
            return open_search_error(
                413,
                "bulk_too_large_exception",
                "bulk action count exceeded configured limit",
                Some("Split the bulk request or raise --max-bulk-actions."),
            );
        }
        let action_value: Value = match serde_json::from_str(action_line) {
            Ok(value) => value,
            Err(error) => {
                return parse_error(format!("bulk action line is not valid JSON: {error}"));
            }
        };
        let (action, meta) = match action_entry(&action_value) {
            Ok(entry) => entry,
            Err(error) => return parse_error(error.reason),
        };
        let index = meta
            .get("_index")
            .and_then(Value::as_str)
            .or(path_index)
            .unwrap_or("")
            .to_string();
        let id = meta
            .get("_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(crate::storage::Store::generated_id);
        match action {
            "index" => {
                let source = parse_bulk_source(lines.next());
                if index.trim().is_empty() {
                    plans.push(BulkPlan::Immediate(json!({ action: bulk_error(StoreError::new(400, "index_missing_exception", "bulk item requires _index or a path index")) })));
                } else if let Err(error) = source {
                    plans.push(BulkPlan::Immediate(json!({ action: bulk_error(error) })));
                } else {
                    let source = source.expect("checked above");
                    plans.push(BulkPlan::Store(BulkStorePlan {
                        action: action.to_string(),
                        index: index.clone(),
                        id: id.clone(),
                        operation: WriteOperation::IndexDocument { index, id, source },
                    }));
                }
            }
            "create" => {
                let source = parse_bulk_source(lines.next());
                if index.trim().is_empty() {
                    plans.push(BulkPlan::Immediate(json!({ action: bulk_error(StoreError::new(400, "index_missing_exception", "bulk item requires _index or a path index")) })));
                } else if let Err(error) = source {
                    plans.push(BulkPlan::Immediate(json!({ action: bulk_error(error) })));
                } else {
                    let source = source.expect("checked above");
                    plans.push(BulkPlan::Store(BulkStorePlan {
                        action: action.to_string(),
                        index: index.clone(),
                        id: id.clone(),
                        operation: WriteOperation::CreateDocument { index, id, source },
                    }));
                }
            }
            "update" => {
                let body = parse_bulk_source(lines.next());
                if index.trim().is_empty() {
                    plans.push(BulkPlan::Immediate(json!({ action: bulk_error(StoreError::new(400, "index_missing_exception", "bulk item requires _index or a path index")) })));
                } else if let Err(error) = body {
                    plans.push(BulkPlan::Immediate(json!({ action: bulk_error(error) })));
                } else {
                    let body = body.expect("checked above");
                    match parse_update_body(&body) {
                        Ok(update) => {
                            plans.push(BulkPlan::Store(BulkStorePlan {
                                action: action.to_string(),
                                index: index.clone(),
                                id: id.clone(),
                                operation: WriteOperation::UpdateDocument {
                                    index,
                                    id,
                                    doc: update.doc,
                                    doc_as_upsert: update.doc_as_upsert,
                                    upsert: update.upsert,
                                },
                            }));
                        }
                        Err(error) => {
                            plans.push(BulkPlan::Immediate(json!({ action: bulk_error(error) })));
                        }
                    }
                }
            }
            "delete" => {
                if index.trim().is_empty() {
                    plans.push(BulkPlan::Immediate(json!({ "delete": bulk_error(StoreError::new(400, "index_missing_exception", "bulk item requires _index or a path index")) })));
                } else {
                    plans.push(BulkPlan::Store(BulkStorePlan {
                        action: action.to_string(),
                        index: index.clone(),
                        id: id.clone(),
                        operation: WriteOperation::DeleteDocument { index, id },
                    }));
                }
            }
            other => {
                plans.push(BulkPlan::Immediate(json!({ other: {"status": 400, "error": {"type": "illegal_argument_exception", "reason": "unsupported bulk action"}} })));
            }
        }
    }

    let operations = plans
        .iter()
        .filter_map(|plan| match plan {
            BulkPlan::Store(plan) => Some(plan.operation.clone()),
            BulkPlan::Immediate(_) => None,
        })
        .collect::<Vec<_>>();
    let mut outcomes = if operations.is_empty() {
        Vec::new().into_iter()
    } else {
        match run_store(state.store.clone(), move |store| {
            store.apply_write_operations(operations)
        })
        .await
        {
            Ok(results) => results.into_iter(),
            Err(error) => return store_error(error),
        }
    };

    let items = plans
        .into_iter()
        .map(|plan| match plan {
            BulkPlan::Immediate(value) => value,
            BulkPlan::Store(plan) => match outcomes.next().expect("one outcome per store plan") {
                Ok(WriteOutcome::Document(doc)) => {
                    let (status, result) = if plan.action == "create" {
                        (201, "created")
                    } else if plan.action == "update" {
                        (
                            if doc.version == 1 { 201 } else { 200 },
                            if doc.version == 1 {
                                "created"
                            } else {
                                "updated"
                            },
                        )
                    } else if doc.version == 1 {
                        (201, "created")
                    } else {
                        (200, "updated")
                    };
                    json!({ plan.action: bulk_doc_result(&plan.index, &plan.id, &doc, status, result) })
                }
                Ok(WriteOutcome::Deleted { found }) => json!({
                    "delete": {
                        "_index": plan.index,
                        "_id": plan.id,
                        "status": if found { 200 } else { 404 },
                        "result": if found { "deleted" } else { "not_found" }
                    }
                }),
                Err(error) => json!({ plan.action: bulk_error(error) }),
            },
        })
        .collect::<Vec<_>>();

    let errors = items.iter().any(|item| {
        item.as_object()
            .and_then(|object| object.values().next())
            .and_then(|value| value.get("status"))
            .and_then(Value::as_u64)
            .map(|status| status >= 400)
            .unwrap_or(false)
    });

    Response::json(200, json!({ "took": 0, "errors": errors, "items": items }))
}

fn handle_search(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
    if let Err(error) = search_engine::limits::validate_body_bytes(request.body.len()) {
        return open_search_error(
            413,
            "content_too_long_exception",
            error,
            Some("Reduce the search request size."),
        );
    }
    let mut body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    if let Some(source_filter) = source_filter_from_query(&request.query) {
        body["_source"] = source_filter;
    }
    let from = numeric_param(&body, &request.query, "from", 0);
    let size = numeric_param(&body, &request.query, "size", 10);
    let scroll_requested = request.query_value("scroll").is_some() || body.get("scroll").is_some();
    if let Err(error) = search_engine::limits::validate_request(
        &body,
        from,
        size,
        search_engine::limits::SearchLimits {
            max_result_window: state.config.max_result_window,
        },
    ) {
        let error_type = search_validation_error_type(&error);
        return open_search_error(
            400,
            error_type,
            error,
            Some("Use a narrower query for OpenSearch Lite or raise the relevant local limit."),
        );
    }
    if scroll_requested && size > MAX_SCROLL_RETAINED_HITS {
        return open_search_error(
            400,
            "resource_limit_exception",
            format!(
                "scroll page size [{size}] exceeds the local scroll page limit [{MAX_SCROLL_RETAINED_HITS}]"
            ),
            Some("Use a smaller scroll size for OpenSearch Lite."),
        );
    }
    let indices = path_indices(path_index, "_search");
    match state
        .store
        .read_database(|db| validate_search_indices(db, &indices))
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return store_error(error),
        Err(error) => return store_error(error),
    }
    let search_result = state.store.read_database(|db| {
        search_engine::search(
            db,
            SearchRequest {
                indices,
                body,
                from,
                size: if scroll_requested {
                    scroll_capture_size(state, size)
                } else {
                    size
                },
            },
        )
    });
    match search_result {
        Ok(Ok(mut body)) => {
            if scroll_requested {
                body = match scroll_response(state, body, size) {
                    Ok(body) => body,
                    Err(response) => return response,
                };
            }
            if request.query_value("rest_total_hits_as_int") == Some("true") {
                if let Some(total) = body["hits"]["total"]["value"].as_u64() {
                    body["hits"]["total"] = json!(total);
                }
            }
            Response::json(200, body)
        }
        Ok(Err(error)) => open_search_error(
            400,
            "x_content_parse_exception",
            error,
            Some("Use match_all, term, terms, range, exists, bool, ids, or simple match queries."),
        ),
        Err(error) => open_search_error(
            error.status,
            error.error_type,
            error.reason,
            Some("Retry the search request."),
        ),
    }
}

fn scroll_capture_size(state: &AppState, batch_size: usize) -> usize {
    state
        .config
        .max_result_window
        .min(MAX_SCROLL_RETAINED_HITS)
        .max(batch_size.max(1))
}

fn scroll_response(
    state: &AppState,
    mut body: Value,
    batch_size: usize,
) -> Result<Value, Response> {
    let hits = body["hits"]["hits"].as_array().cloned().unwrap_or_default();
    let total = body["hits"]["total"].clone();
    let max_score = body["hits"]["max_score"].clone();
    let page = state
        .runtime
        .create_scroll(
            hits,
            total,
            max_score,
            batch_size,
            scroll_memory_budget(state),
        )
        .map_err(|error| {
            open_search_error(
                error.status,
                error.error_type,
                error.reason,
                Some("Clear old scroll contexts or use a narrower query."),
            )
        })?;
    body["_scroll_id"] = json!(page.scroll_id);
    body["hits"]["total"] = page.total;
    body["hits"]["max_score"] = page.max_score;
    body["hits"]["hits"] = Value::Array(page.hits);
    Ok(body)
}

fn scroll_memory_budget(state: &AppState) -> usize {
    state
        .config
        .memory_limit_bytes
        .min(MAX_SCROLL_RETAINED_BYTES)
}

fn handle_scroll(state: &AppState, request: &Request, path_scroll_id: Option<&str>) -> Response {
    let scroll_id = match path_scroll_id
        .map(decode_path_param)
        .or_else(|| scroll_id_from_request(request))
    {
        Some(scroll_id) => scroll_id,
        None => return parse_error("scroll request requires [scroll_id]".to_string()),
    };
    match state.runtime.next_scroll(&scroll_id) {
        Some(page) => Response::json(
            200,
            json!({
                "_scroll_id": page.scroll_id,
                "took": 0,
                "timed_out": false,
                "_shards": { "total": 1, "successful": 1, "skipped": 0, "failed": 0 },
                "hits": {
                    "total": page.total,
                    "max_score": page.max_score,
                    "hits": page.hits
                }
            }),
        ),
        None => open_search_error(
            404,
            "search_context_missing_exception",
            format!("No search context found for id [{scroll_id}]"),
            Some("Start a new search with a scroll parameter and use the returned _scroll_id."),
        ),
    }
}

fn handle_clear_scroll(
    state: &AppState,
    request: &Request,
    path_scroll_id: Option<&str>,
) -> Response {
    let mut scroll_ids = Vec::new();
    if let Some(path_scroll_id) = path_scroll_id {
        scroll_ids.push(decode_path_param(path_scroll_id));
    }
    scroll_ids.extend(scroll_ids_from_request(request));
    if scroll_ids.is_empty() {
        return parse_error("clear scroll requires [scroll_id]".to_string());
    }
    let num_freed = state.runtime.clear_scrolls(&scroll_ids);
    Response::json(
        200,
        json!({
            "succeeded": true,
            "num_freed": num_freed
        }),
    )
}

fn handle_task_get(state: &AppState, task_id: &str) -> Response {
    match state.runtime.task(task_id) {
        Some(task) => Response::json(200, task.response_body()),
        None => open_search_error(
            404,
            "resource_not_found_exception",
            format!("task [{task_id}] is missing"),
            Some("Only completed local tasks created during this process are available."),
        ),
    }
}

fn handle_count(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
    if let Err(error) = search_engine::limits::validate_body_bytes(request.body.len()) {
        return open_search_error(
            413,
            "content_too_long_exception",
            error,
            Some("Reduce the count request size."),
        );
    }
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    if let Err(error) = search_engine::limits::validate_request(
        &body,
        0,
        0,
        search_engine::limits::SearchLimits {
            max_result_window: state.config.max_result_window,
        },
    ) {
        let error_type = search_validation_error_type(&error);
        return open_search_error(
            400,
            error_type,
            error,
            Some("Use a narrower query for OpenSearch Lite."),
        );
    }
    let indices = path_indices(path_index, "_count");
    match state
        .store
        .read_database(|db| validate_search_indices(db, &indices))
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return store_error(error),
        Err(error) => return store_error(error),
    }
    let search_result = state.store.read_database(|db| {
        search_engine::search(
            db,
            SearchRequest {
                indices,
                body,
                from: 0,
                size: 0,
            },
        )
    });
    match search_result {
        Ok(Ok(body)) => Response::json(
            200,
            json!({
                "count": body["hits"]["total"]["value"].as_u64().unwrap_or(0),
                "_shards": body["_shards"].clone()
            }),
        ),
        Ok(Err(error)) => open_search_error(
            400,
            "x_content_parse_exception",
            error,
            Some("Use match_all, term, terms, range, exists, bool, ids, or simple match queries."),
        ),
        Err(error) => store_error(error),
    }
}

async fn handle_delete_by_query(
    state: &AppState,
    request: &Request,
    path_index: Option<&str>,
) -> Response {
    let (_body, query) =
        match parse_by_query_request(state, request, ByQueryQueryMode::RequireQuery) {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let indices = path_indices(path_index, "_delete_by_query");
    let result = apply_dynamic_bulk_by_query_operations(state, move |db| {
        let matched = matching_documents(db, &indices, &query)?;
        let total = matched.len();
        let operations = matched
            .into_iter()
            .map(|doc| WriteOperation::DeleteDocument {
                index: doc.index,
                id: doc.id,
            })
            .collect::<Vec<_>>();
        Ok((
            operations,
            ByQueryPlan {
                total,
                ..Default::default()
            },
        ))
    })
    .await;
    let (outcome, plan) = match result {
        Ok(result) => result,
        Err(error) => return store_error(error),
    };
    Response::json(
        200,
        bulk_by_query_response(
            plan.total,
            0,
            0,
            outcome.deleted,
            0,
            outcome.version_conflicts,
        ),
    )
}

async fn handle_update_by_query(
    state: &AppState,
    request: &Request,
    path_index: Option<&str>,
) -> Response {
    let (body, query) =
        match parse_by_query_request(state, request, ByQueryQueryMode::DefaultMatchAll) {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let Some(script) = body.get("script") else {
        return open_search_error(
            400,
            "script_exception",
            "update_by_query requires a supported script in OpenSearch Lite",
            Some("Use delete_by_query for pure deletes, or use the saved-object namespace/workspace removal script shape."),
        );
    };
    let script = match parse_saved_object_update_script(script) {
        Ok(script) => script,
        Err(error) => return update_by_query_script_error(error),
    };
    let indices = path_indices(path_index, "_update_by_query");
    let result = apply_dynamic_bulk_by_query_operations(state, move |db| {
        let matched = matching_documents(db, &indices, &query)?;
        let total = matched.len();
        let mut noops = 0usize;
        let mut operations = Vec::new();
        for doc in matched {
            match apply_saved_object_update_script(&doc.source, &script) {
                Ok(UpdateByQueryAction::Delete) => {
                    operations.push(WriteOperation::DeleteDocument {
                        index: doc.index,
                        id: doc.id,
                    })
                }
                Ok(UpdateByQueryAction::Index(source)) => {
                    operations.push(WriteOperation::IndexDocument {
                        index: doc.index,
                        id: doc.id,
                        source,
                    })
                }
                Ok(UpdateByQueryAction::Noop) => noops += 1,
                Err(error) => return Err(error),
            }
        }
        Ok((
            operations,
            ByQueryPlan {
                total,
                noops,
                ..Default::default()
            },
        ))
    })
    .await;
    let (outcome, plan) = match result {
        Ok(result) => result,
        Err(error) => return store_error(error),
    };
    Response::json(
        200,
        bulk_by_query_response(
            plan.total,
            outcome.updated,
            0,
            outcome.deleted,
            plan.noops,
            outcome.version_conflicts,
        ),
    )
}

async fn handle_reindex(state: &AppState, request: &Request) -> Response {
    if let Err(error) = search_engine::limits::validate_body_bytes(request.body.len()) {
        return open_search_error(
            413,
            "content_too_long_exception",
            error,
            Some("Reduce the reindex request size."),
        );
    }
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    let source_indices = match reindex_source_indices(&body) {
        Ok(indices) => indices,
        Err(error) => return store_error(error),
    };
    let dest_index = match body
        .get("dest")
        .and_then(|dest| dest.get("index"))
        .and_then(Value::as_str)
    {
        Some(index) if !index.trim().is_empty() => index.to_string(),
        _ => {
            return store_error(StoreError::new(
                400,
                "illegal_argument_exception",
                "reindex requires dest.index",
            ));
        }
    };
    let op_type = match reindex_op_type(&body) {
        Ok(op_type) => op_type,
        Err(error) => return store_error(error),
    };
    let conflicts_proceed = request.query_value("conflicts") == Some("proceed")
        || body.get("conflicts").and_then(Value::as_str) == Some("proceed");
    let query = body
        .get("source")
        .and_then(|source| source.get("query"))
        .cloned()
        .unwrap_or_else(|| json!({"match_all": {}}));
    if let Err(error) = validate_query_only(state, request.body.len(), &query) {
        return error;
    }
    let script = body.get("script").cloned();
    let dest_index_for_plan = dest_index.clone();
    let result = apply_dynamic_bulk_by_query_operations(state, move |db| {
        let matched = matching_documents(db, &source_indices, &query)?;
        let total = matched.len();
        let mut operations = Vec::new();
        let mut planned_create_ids = BTreeSet::new();
        let mut version_conflicts = 0usize;
        for doc in matched {
            let id = reindex_document_id(&doc, script.as_ref())?;
            if op_type == ReindexOpType::Create {
                let already_exists = document_exists(db, &dest_index_for_plan, &id)
                    || !planned_create_ids.insert(id.clone());
                if already_exists {
                    version_conflicts += 1;
                    if !conflicts_proceed {
                        return Err(StoreError::new(
                            409,
                            "version_conflict_engine_exception",
                            format!("document [{id}] already exists"),
                        ));
                    }
                    continue;
                }
            }
            let operation = match op_type {
                ReindexOpType::Index => WriteOperation::IndexDocument {
                    index: dest_index_for_plan.clone(),
                    id,
                    source: doc.source,
                },
                ReindexOpType::Create => WriteOperation::CreateDocument {
                    index: dest_index_for_plan.clone(),
                    id,
                    source: doc.source,
                },
            };
            operations.push(operation);
        }
        Ok((
            operations,
            ByQueryPlan {
                total,
                version_conflicts,
                ..Default::default()
            },
        ))
    })
    .await;
    let (mut outcome, plan) = match result {
        Ok(result) => result,
        Err(error) => return store_error(error),
    };
    outcome.version_conflicts += plan.version_conflicts;
    let response = bulk_by_query_response(
        plan.total,
        outcome.updated,
        outcome.created,
        0,
        0,
        outcome.version_conflicts,
    );
    if request.query_value("wait_for_completion") == Some("false") {
        let task = state
            .runtime
            .record_completed_task("indices:data/write/reindex", response);
        return Response::json(200, json!({ "task": task }));
    }
    Response::json(200, response)
}

fn handle_mget(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    let path_index = path_index.filter(|index| *index != "_mget");
    let docs = mget_items(&body, path_index, source_filter_from_query(&request.query));
    let result = state.store.read_database(|db| {
        docs.into_iter()
            .map(|item| mget_doc_response(db, item))
            .collect::<Vec<_>>()
    });
    match result {
        Ok(docs) => Response::json(200, json!({ "docs": docs })),
        Err(error) => store_error(error),
    }
}

fn handle_refresh(state: &AppState, path_index: Option<&str>) -> Response {
    let indices = path_indices(path_index, "_refresh");
    match state.store.read_database(|db| {
        let names = resolve_index_patterns(db, &indices)?;
        Ok(names
            .iter()
            .filter_map(|name| db.indexes.get(name))
            .map(index_total_shards)
            .sum::<u64>())
    }) {
        Ok(Ok(total_shards)) => Response::json(
            200,
            json!({
                "_shards": {
                    "total": total_shards,
                    "successful": total_shards,
                    "failed": 0
                }
            }),
        ),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

fn handle_msearch(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
    if let Err(error) = search_engine::limits::validate_body_bytes(request.body.len()) {
        return open_search_error(
            413,
            "content_too_long_exception",
            error,
            Some("Reduce the msearch request size."),
        );
    }
    let text = match std::str::from_utf8(&request.body) {
        Ok(text) => text,
        Err(error) => return parse_error(format!("msearch body must be UTF-8 NDJSON: {error}")),
    };
    let mut responses = Vec::new();
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    while let Some(header_line) = lines.next() {
        let header = match serde_json::from_str::<Value>(header_line) {
            Ok(header) => header,
            Err(error) => return parse_error(format!("msearch header is not valid JSON: {error}")),
        };
        let Some(body_line) = lines.next() else {
            return parse_error("msearch header requires a body line".to_string());
        };
        let body = match serde_json::from_str::<Value>(body_line) {
            Ok(body) => body,
            Err(error) => return parse_error(format!("msearch body is not valid JSON: {error}")),
        };
        let from = body
            .get("from")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0);
        let size = body
            .get("size")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(10);
        let indices = msearch_indices(&header, path_index);
        let validation_error = search_engine::limits::validate_request(
            &body,
            from,
            size,
            search_engine::limits::SearchLimits {
                max_result_window: state.config.max_result_window,
            },
        )
        .err();
        let response = state.store.read_database(|db| {
            if let Some(error) = validation_error {
                return Err(StoreError::new(
                    400,
                    search_validation_error_type(&error),
                    error,
                ));
            }
            validate_search_indices(db, &indices).and_then(|_| {
                search_engine::search(
                    db,
                    SearchRequest {
                        indices,
                        body,
                        from,
                        size,
                    },
                )
                .map_err(|reason| StoreError::new(400, "x_content_parse_exception", reason))
            })
        });
        match response {
            Ok(Ok(body)) => responses.push(body),
            Ok(Err(error)) | Err(error) => responses.push(json!({
                "error": {
                    "type": error.error_type,
                    "reason": error.reason
                },
                "status": error.status
            })),
        }
    }
    Response::json(200, json!({ "responses": responses }))
}

#[derive(Debug, Clone)]
struct MatchedDoc {
    index: String,
    id: String,
    source: Value,
}

#[derive(Debug, Default)]
struct BulkByQueryOutcome {
    created: usize,
    updated: usize,
    deleted: usize,
    version_conflicts: usize,
}

enum UpdateByQueryAction {
    Delete,
    Index(Value),
    Noop,
}

#[derive(Debug, Clone, Copy)]
enum ByQueryQueryMode {
    RequireQuery,
    DefaultMatchAll,
}

#[derive(Debug, Clone)]
enum SavedObjectListRemoval {
    Namespace { namespace: String },
    Workspace { workspace: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReindexOpType {
    Index,
    Create,
}

#[derive(Debug, Default)]
struct ByQueryPlan {
    total: usize,
    noops: usize,
    version_conflicts: usize,
}

fn parse_by_query_request(
    state: &AppState,
    request: &Request,
    query_mode: ByQueryQueryMode,
) -> Result<(Value, Value), Response> {
    if let Err(error) = search_engine::limits::validate_body_bytes(request.body.len()) {
        return Err(open_search_error(
            413,
            "content_too_long_exception",
            error,
            Some("Reduce the by-query request size."),
        ));
    }
    let body = request.body_json().map_err(parse_error)?;
    let query = match body.get("query") {
        Some(query) => query.clone(),
        None if matches!(query_mode, ByQueryQueryMode::DefaultMatchAll) => {
            json!({"match_all": {}})
        }
        None => {
            return Err(open_search_error(
                400,
                "action_request_validation_exception",
                "Validation Failed: 1: query is missing;",
                Some("Provide an explicit query object; use match_all only when you intend to affect every matching document."),
            ));
        }
    };
    validate_query_only(state, request.body.len(), &query)?;
    Ok((body, query))
}

fn validate_query_only(state: &AppState, body_len: usize, query: &Value) -> Result<(), Response> {
    if let Err(error) = search_engine::limits::validate_body_bytes(body_len) {
        return Err(open_search_error(
            413,
            "content_too_long_exception",
            error,
            Some("Reduce the request size."),
        ));
    }
    let body = json!({ "query": query });
    search_engine::limits::validate_request(
        &body,
        0,
        0,
        search_engine::limits::SearchLimits {
            max_result_window: state.config.max_result_window,
        },
    )
    .map_err(|error| {
        open_search_error(
            400,
            search_validation_error_type(&error),
            error,
            Some("Use a narrower query for OpenSearch Lite."),
        )
    })
}

fn matching_documents(
    db: &Database,
    requested: &[String],
    query: &Value,
) -> StoreResult<Vec<MatchedDoc>> {
    let names = resolve_index_patterns(db, requested)?;
    let mut matches = Vec::new();
    for index_name in names {
        let Some(index) = db.indexes.get(&index_name) else {
            continue;
        };
        for doc in index.documents.values() {
            let matched = search_engine::evaluator::document_matches(doc, query)
                .map_err(|error| StoreError::new(400, "x_content_parse_exception", error))?;
            if matched {
                matches.push(MatchedDoc {
                    index: index_name.clone(),
                    id: doc.id.clone(),
                    source: doc.source.clone(),
                });
            }
        }
    }
    Ok(matches)
}

async fn apply_dynamic_bulk_by_query_operations<T>(
    state: &AppState,
    build_operations: impl FnOnce(&Database) -> StoreResult<(Vec<WriteOperation>, T)> + Send + 'static,
) -> StoreResult<(BulkByQueryOutcome, T)>
where
    T: Send + 'static,
{
    let (results, metadata) = run_store(state.store.clone(), move |store| {
        store.apply_dynamic_write_operations_atomic(build_operations)
    })
    .await?;
    let mut outcome = BulkByQueryOutcome::default();
    for result in results {
        match result {
            WriteOutcome::Document(doc) => {
                if doc.version == 1 {
                    outcome.created += 1;
                } else {
                    outcome.updated += 1;
                }
            }
            WriteOutcome::Deleted { found } => {
                if found {
                    outcome.deleted += 1;
                }
            }
        }
    }
    Ok((outcome, metadata))
}

fn bulk_by_query_response(
    total: usize,
    updated: usize,
    created: usize,
    deleted: usize,
    noops: usize,
    version_conflicts: usize,
) -> Value {
    json!({
        "took": 0,
        "timed_out": false,
        "total": total,
        "updated": updated,
        "created": created,
        "deleted": deleted,
        "batches": if total == 0 { 0 } else { 1 },
        "version_conflicts": version_conflicts,
        "noops": noops,
        "retries": {
            "bulk": 0,
            "search": 0
        },
        "throttled_millis": 0,
        "requests_per_second": -1.0,
        "throttled_until_millis": 0,
        "failures": [],
        "_shards": {
            "total": 1,
            "successful": 1,
            "failed": 0
        }
    })
}

fn parse_saved_object_update_script(script: &Value) -> StoreResult<SavedObjectListRemoval> {
    let script_source = script
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| StoreError::new(400, "script_exception", "script.source is required"))?;
    if let Some(lang) = script.get("lang").and_then(Value::as_str) {
        if lang != "painless" {
            return Err(StoreError::new(
                400,
                "script_exception",
                "saved-object update_by_query scripts must use painless",
            ));
        }
    }
    let normalized = normalize_script_source(script_source);
    if normalized == saved_object_removal_script("namespaces", "namespace") {
        let namespace = script_param(script, "namespace")?;
        return Ok(SavedObjectListRemoval::Namespace { namespace });
    }
    if normalized == saved_object_removal_script("workspaces", "workspace") {
        let workspace = script_param(script, "workspace")?;
        return Ok(SavedObjectListRemoval::Workspace { workspace });
    }
    Err(StoreError::new(
        400,
        "script_exception",
        "unsupported update_by_query script for OpenSearch Lite",
    ))
}

fn update_by_query_script_error(error: StoreError) -> Response {
    open_search_error(
        error.status,
        error.error_type,
        error.reason,
        Some("Use the saved-object namespace/workspace removal script shape from the migration tests, or simplify the request."),
    )
}

fn normalize_script_source(script_source: &str) -> String {
    script_source
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| if character == '"' { '\'' } else { character })
        .collect()
}

fn saved_object_removal_script(field: &str, param: &str) -> String {
    format!(
        "if(!ctx._source.containsKey('{field}')){{ctx.op='delete';}}else{{ctx._source['{field}'].removeAll(Collections.singleton(params['{param}']));if(ctx._source['{field}'].empty){{ctx.op='delete';}}}}"
    )
}

fn script_param(script: &Value, name: &'static str) -> StoreResult<String> {
    let params = script.get("params").unwrap_or(&Value::Null);
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            StoreError::new(
                400,
                "script_exception",
                format!("saved-object removal script requires params.{name}"),
            )
        })
}

fn apply_saved_object_update_script(
    source: &Value,
    script: &SavedObjectListRemoval,
) -> StoreResult<UpdateByQueryAction> {
    match script {
        SavedObjectListRemoval::Namespace { namespace } => {
            remove_saved_object_list_value(source, "namespaces", namespace.as_str())
        }
        SavedObjectListRemoval::Workspace { workspace } => {
            remove_saved_object_list_value(source, "workspaces", workspace.as_str())
        }
    }
}

fn remove_saved_object_list_value(
    source: &Value,
    field: &str,
    target: &str,
) -> StoreResult<UpdateByQueryAction> {
    let Some(object) = source.as_object() else {
        return Err(StoreError::new(
            400,
            "script_exception",
            "saved-object update script requires an object source",
        ));
    };
    let Some(value) = object.get(field) else {
        return Ok(UpdateByQueryAction::Delete);
    };
    let Some(values) = value.as_array() else {
        return Err(StoreError::new(
            400,
            "script_exception",
            format!("saved-object field [{field}] must be an array"),
        ));
    };
    let retained = values
        .iter()
        .filter(|value| value.as_str() != Some(target))
        .cloned()
        .collect::<Vec<_>>();
    if retained.len() == values.len() {
        return Ok(UpdateByQueryAction::Noop);
    }
    if retained.is_empty() {
        return Ok(UpdateByQueryAction::Delete);
    }
    let mut source = source.clone();
    source[field] = Value::Array(retained);
    Ok(UpdateByQueryAction::Index(source))
}

fn reindex_source_indices(body: &Value) -> StoreResult<Vec<String>> {
    let Some(source) = body.get("source") else {
        return Err(StoreError::new(
            400,
            "illegal_argument_exception",
            "reindex requires source.index",
        ));
    };
    match source.get("index") {
        Some(Value::String(index)) if !index.trim().is_empty() => {
            Ok(index.split(',').map(ToString::to_string).collect())
        }
        Some(Value::Array(indices)) => {
            let indices = indices
                .iter()
                .filter_map(Value::as_str)
                .filter(|index| !index.trim().is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if indices.is_empty() {
                Err(StoreError::new(
                    400,
                    "illegal_argument_exception",
                    "reindex source.index must not be empty",
                ))
            } else {
                Ok(indices)
            }
        }
        _ => Err(StoreError::new(
            400,
            "illegal_argument_exception",
            "reindex requires source.index",
        )),
    }
}

fn reindex_op_type(body: &Value) -> StoreResult<ReindexOpType> {
    match body
        .get("dest")
        .and_then(|dest| dest.get("op_type"))
        .and_then(Value::as_str)
    {
        None | Some("index") => Ok(ReindexOpType::Index),
        Some("create") => Ok(ReindexOpType::Create),
        Some(op_type) => Err(StoreError::new(
            400,
            "illegal_argument_exception",
            format!("unsupported reindex dest.op_type [{op_type}]"),
        )),
    }
}

fn document_exists(db: &Database, index_or_alias: &str, id: &str) -> bool {
    db.resolve_index(index_or_alias)
        .and_then(|index| {
            db.indexes
                .get(&index)
                .and_then(|index| index.documents.get(id))
        })
        .is_some()
}

fn reindex_document_id(doc: &MatchedDoc, script: Option<&Value>) -> StoreResult<String> {
    let Some(script) = script else {
        return Ok(doc.id.clone());
    };
    let script_source = script
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| StoreError::new(400, "script_exception", "script.source is required"))?;
    if script_source.trim() == "ctx._id = ctx._source.type + ':' + ctx._id" {
        let doc_type = doc
            .source
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                StoreError::new(
                    400,
                    "script_exception",
                    "reindex id rewrite script requires _source.type",
                )
            })?;
        return Ok(format!("{doc_type}:{}", doc.id));
    }
    Err(StoreError::new(
        400,
        "script_exception",
        "unsupported reindex script for OpenSearch Lite",
    ))
}

fn handle_stats(state: &AppState, parts: &[&str]) -> Response {
    let (path_index, metrics) = stats_path(parts);
    let indices = path_indices(path_index, "_stats");
    let metrics = match parse_stats_metrics(metrics) {
        Ok(metrics) => metrics,
        Err(error) => return store_error(error),
    };
    match state
        .store
        .read_database(|db| index_stats(db, &indices, &metrics))
    {
        Ok(Ok(body)) => Response::json(200, body),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

fn handle_field_mapping(state: &AppState, request: &Request, parts: &[&str]) -> Response {
    let (path_index, fields) = if parts.first() == Some(&"_mapping") {
        (None, parts.get(2).copied())
    } else {
        (parts.first().copied(), parts.get(3).copied())
    };
    let Some(fields) = fields else {
        return parse_error("field mapping requires field names".to_string());
    };
    let fields = fields
        .split(',')
        .filter(|field| !field.trim().is_empty())
        .map(|field| field.trim().to_string())
        .collect::<Vec<_>>();
    let include_defaults = request.query_value("include_defaults") == Some("true");
    let indices = path_indices(path_index, "_mapping");
    match state
        .store
        .read_database(|db| field_mapping_response(db, &indices, &fields, include_defaults))
    {
        Ok(Ok(body)) => Response::json(200, body),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

fn cat_indices(state: &AppState, request: &Request, api_name: &str) -> Response {
    let parts = segments(&request.path);
    let path_index = parts.get(2).copied();
    match state.store.read_database(|db| {
        let names = resolve_index_patterns(db, &path_indices(path_index, "_cat/indices"))?;
        let rows = names
            .iter()
            .filter_map(|name| db.indexes.get(name))
            .map(|index| {
                let store_bytes = index.store_size_bytes;
                json!({
                    "health": "green",
                    "status": "open",
                    "index": index.name,
                    "uuid": format!("opensearch-lite-{}", index.name),
                    "pri": index_setting_u64(index, "number_of_shards", 1).to_string(),
                    "rep": index_setting_u64(index, "number_of_replicas", 0).to_string(),
                    "docs.count": index.documents.len().to_string(),
                    "docs.deleted": index.tombstones.len().to_string(),
                    "store.size": format!("{store_bytes}b"),
                    "pri.store.size": format!("{store_bytes}b")
                })
            })
            .collect::<Vec<_>>();
        Ok(Value::Array(rows))
    }) {
        Ok(Ok(rows)) => Response::json(200, rows).compatibility_signal(api_name, "best_effort"),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

fn cluster_stats_response(db: &Database) -> Value {
    let docs = db.document_count();
    let deleted = db
        .indexes
        .values()
        .map(|index| index.tombstones.len())
        .sum::<usize>();
    let store_bytes = db
        .indexes
        .values()
        .map(|index| index.store_size_bytes)
        .sum::<usize>();
    let shard_count = db.indexes.values().map(index_total_shards).sum::<u64>();

    json!({
        "cluster_name": "opensearch-lite",
        "cluster_uuid": format!("opensearch-lite-{:x}", db.indexes.len()),
        "timestamp": 0u64,
        "status": "green",
        "indices": {
            "count": db.indexes.len(),
            "shards": {
                "total": shard_count,
                "primaries": db.indexes.len()
            },
            "docs": {
                "count": docs,
                "deleted": deleted
            },
            "store": {
                "size_in_bytes": store_bytes,
                "reserved_in_bytes": 0
            }
        },
        "nodes": {
            "count": {
                "total": 1,
                "cluster_manager": 1,
                "data": 1,
                "ingest": 1
            },
            "versions": ["opensearch-lite"],
            "os": {},
            "process": {},
            "jvm": {},
            "fs": {
                "total_in_bytes": store_bytes,
                "free_in_bytes": 0,
                "available_in_bytes": 0
            }
        }
    })
}

fn index_stats(db: &Database, requested: &[String], metrics: &[StatsMetric]) -> StoreResult<Value> {
    let names = resolve_index_patterns(db, requested)?;
    let mut indices = serde_json::Map::new();
    let mut all_docs = 0usize;
    let mut all_deleted = 0usize;
    let mut all_store = 0usize;
    let mut total_shards = 0u64;

    for name in names {
        let Some(index) = db.indexes.get(&name) else {
            continue;
        };
        let store_bytes = index.store_size_bytes;
        let stats = stats_block(index, store_bytes, metrics);
        all_docs += index.documents.len();
        all_deleted += index.tombstones.len();
        all_store += store_bytes;
        total_shards += index_total_shards(index);
        indices.insert(
            name.clone(),
            json!({
                "uuid": format!("opensearch-lite-{name}"),
                "health": "green",
                "status": "open",
                "primaries": stats,
                "total": stats
            }),
        );
    }

    let all = aggregate_stats_block(all_docs, all_deleted, all_store, metrics);
    Ok(json!({
        "_shards": {
            "total": total_shards,
            "successful": total_shards,
            "failed": 0
        },
        "_all": {
            "primaries": all,
            "total": all
        },
        "indices": indices
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatsMetric {
    Docs,
    Store,
    Indexing,
    Search,
    Get,
}

fn stats_path<'a>(parts: &'a [&'a str]) -> (Option<&'a str>, Option<&'a str>) {
    if parts.first() == Some(&"_stats") {
        (None, parts.get(1).copied())
    } else {
        (parts.first().copied(), parts.get(2).copied())
    }
}

fn parse_stats_metrics(metric_path: Option<&str>) -> StoreResult<Vec<StatsMetric>> {
    let Some(metric_path) = metric_path else {
        return Ok(all_stats_metrics());
    };
    let mut metrics = Vec::new();
    for metric in metric_path.split(',').filter(|metric| !metric.is_empty()) {
        let parsed = match metric {
            "_all" => {
                metrics.extend(all_stats_metrics());
                continue;
            }
            "docs" => StatsMetric::Docs,
            "store" => StatsMetric::Store,
            "indexing" => StatsMetric::Indexing,
            "search" => StatsMetric::Search,
            "get" => StatsMetric::Get,
            other => {
                let suggestion = if other == "fieldata" {
                    " -> did you mean [fielddata]?"
                } else {
                    ""
                };
                return Err(StoreError::new(
                    400,
                    "illegal_argument_exception",
                    format!(
                        "request [/_stats/{other}] contains unrecognized metric: [{other}]{suggestion}"
                    ),
                ));
            }
        };
        if !metrics.contains(&parsed) {
            metrics.push(parsed);
        }
    }
    if metrics.is_empty() {
        Ok(all_stats_metrics())
    } else {
        Ok(metrics)
    }
}

fn all_stats_metrics() -> Vec<StatsMetric> {
    vec![
        StatsMetric::Docs,
        StatsMetric::Store,
        StatsMetric::Indexing,
        StatsMetric::Search,
        StatsMetric::Get,
    ]
}

fn stats_block(
    index: &crate::storage::IndexMetadata,
    store_bytes: usize,
    metrics: &[StatsMetric],
) -> Value {
    aggregate_stats_block(
        index.documents.len(),
        index.tombstones.len(),
        store_bytes,
        metrics,
    )
}

fn aggregate_stats_block(
    docs: usize,
    deleted: usize,
    store_bytes: usize,
    metrics: &[StatsMetric],
) -> Value {
    let mut block = serde_json::Map::new();
    if metrics.contains(&StatsMetric::Docs) {
        block.insert(
            "docs".to_string(),
            json!({ "count": docs, "deleted": deleted }),
        );
    }
    if metrics.contains(&StatsMetric::Store) {
        block.insert(
            "store".to_string(),
            json!({ "size_in_bytes": store_bytes, "reserved_in_bytes": 0 }),
        );
    }
    if metrics.contains(&StatsMetric::Indexing) {
        block.insert(
            "indexing".to_string(),
            json!({ "index_total": docs, "index_time_in_millis": 0 }),
        );
    }
    if metrics.contains(&StatsMetric::Search) {
        block.insert(
            "search".to_string(),
            json!({ "query_total": 0, "query_time_in_millis": 0 }),
        );
    }
    if metrics.contains(&StatsMetric::Get) {
        block.insert(
            "get".to_string(),
            json!({ "total": 0, "time_in_millis": 0 }),
        );
    }
    Value::Object(block)
}

fn field_mapping_response(
    db: &Database,
    requested: &[String],
    fields: &[String],
    include_defaults: bool,
) -> StoreResult<Value> {
    let names = resolve_index_patterns(db, requested)?;
    let mut output = serde_json::Map::new();
    for name in names {
        let Some(index) = db.indexes.get(&name) else {
            continue;
        };
        let mut mappings = serde_json::Map::new();
        for field in fields {
            for (field_name, mut mapping) in field_mappings_for_request(&index.mappings, field) {
                if include_defaults {
                    add_mapping_defaults(&mut mapping);
                }
                mappings.insert(
                    field_name.clone(),
                    json!({
                        "full_name": field_name,
                        "mapping": { field_name: mapping }
                    }),
                );
            }
        }
        output.insert(name.clone(), json!({ "mappings": mappings }));
    }
    Ok(Value::Object(output))
}

fn field_mappings_for_request(mappings: &Value, field: &str) -> Vec<(String, Value)> {
    if field.contains('*') {
        let mut fields = Vec::new();
        collect_mapping_fields(mappings, "", &mut fields);
        fields
            .into_iter()
            .filter(|(field_name, _)| wildcard_matches(field, field_name))
            .collect()
    } else {
        mapping_for_field(mappings, field)
            .map(|mapping| vec![(field.to_string(), mapping)])
            .unwrap_or_default()
    }
}

fn collect_mapping_fields(mappings: &Value, prefix: &str, fields: &mut Vec<(String, Value)>) {
    let Some(properties) = mappings.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, mapping) in properties {
        let full_name = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if mapping.get("type").is_some() || mapping.get("properties").is_none() {
            fields.push((full_name.clone(), mapping.clone()));
        }
        collect_mapping_fields(mapping, &full_name, fields);
    }
}

fn mapping_for_field(mappings: &Value, field: &str) -> Option<Value> {
    let mut properties = mappings.get("properties")?;
    let mut segments = field.split('.').peekable();
    while let Some(segment) = segments.next() {
        let mapping = properties.get(segment)?;
        if segments.peek().is_none() {
            return Some(mapping.clone());
        }
        properties = mapping.get("properties")?;
    }
    None
}

fn source_disabled(mappings: &Value) -> bool {
    mappings
        .get("_source")
        .and_then(|source| source.get("enabled"))
        .and_then(Value::as_bool)
        == Some(false)
}

fn add_mapping_defaults(mapping: &mut Value) {
    if mapping.get("type").and_then(Value::as_str) == Some("text")
        && mapping.get("analyzer").is_none()
    {
        mapping["analyzer"] = json!("default");
    }
}

fn resolve_index_patterns(db: &Database, requested: &[String]) -> StoreResult<Vec<String>> {
    if requested.is_empty()
        || requested
            .iter()
            .any(|index| matches!(index.as_str(), "_all" | "*"))
    {
        return Ok(db.indexes.keys().cloned().collect());
    }
    let mut names = Vec::new();
    for requested in requested {
        if requested.contains('*') {
            names.extend(
                db.indexes
                    .keys()
                    .filter(|name| wildcard_matches(requested, name))
                    .cloned(),
            );
            continue;
        }
        let Some(name) = db.resolve_index(requested) else {
            return Err(StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{requested}]"),
            ));
        };
        names.push(name);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

fn index_setting_u64(index: &crate::storage::IndexMetadata, key: &str, default: u64) -> u64 {
    index
        .settings
        .get("index")
        .and_then(|settings| settings.get(key))
        .or_else(|| index.settings.get(key))
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(default)
}

fn index_total_shards(index: &crate::storage::IndexMetadata) -> u64 {
    let primaries = index_setting_u64(index, "number_of_shards", 1);
    let replicas = index_setting_u64(index, "number_of_replicas", 0);
    primaries.saturating_mul(replicas.saturating_add(1))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut remaining = value;
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            first = false;
            continue;
        }
        if first && !pattern.starts_with('*') {
            let Some(stripped) = remaining.strip_prefix(part) else {
                return false;
            };
            remaining = stripped;
        } else if let Some(position) = remaining.find(part) {
            remaining = &remaining[position + part.len()..];
        } else {
            return false;
        }
        first = false;
    }
    pattern.ends_with('*') || remaining.is_empty()
}

fn doc_write_response(
    index: &str,
    id: &str,
    doc: &crate::storage::StoredDocument,
    status: u16,
    result: &str,
) -> Response {
    Response::json(status, doc_write_body(index, id, doc, result))
}

fn doc_write_body(
    index: &str,
    id: &str,
    doc: &crate::storage::StoredDocument,
    result: &str,
) -> Value {
    json!({
        "_index": index,
        "_id": id,
        "_version": doc.version,
        "result": result,
        "_shards": { "total": 1, "successful": 1, "failed": 0 },
        "_seq_no": doc.seq_no,
        "_primary_term": doc.primary_term
    })
}

fn bulk_doc_result(
    index: &str,
    id: &str,
    doc: &crate::storage::StoredDocument,
    status: u16,
    result: &str,
) -> Value {
    json!({
        "_index": index,
        "_id": id,
        "_version": doc.version,
        "result": result,
        "status": status,
        "_seq_no": doc.seq_no,
        "_primary_term": doc.primary_term
    })
}

fn bulk_error(error: StoreError) -> Value {
    json!({
        "status": error.status,
        "error": {
            "type": error.error_type,
            "reason": error.reason
        }
    })
}

fn bulk_parse_error(reason: impl Into<String>) -> Value {
    bulk_error(StoreError::new(400, "parse_exception", reason))
}

#[derive(Debug)]
enum BulkPlan {
    Immediate(Value),
    Store(BulkStorePlan),
}

#[derive(Debug)]
struct BulkStorePlan {
    action: String,
    index: String,
    id: String,
    operation: WriteOperation,
}

fn parse_bulk_source(line: Option<&str>) -> StoreResult<Value> {
    let Some(line) = line else {
        return Err(StoreError::new(
            400,
            "parse_exception",
            "bulk action requires a source line",
        ));
    };
    if line.trim().is_empty() {
        return Err(StoreError::new(
            400,
            "parse_exception",
            "bulk source line must not be empty",
        ));
    }
    serde_json::from_str::<Value>(line)
        .map_err(|error| StoreError::new(400, "parse_exception", error.to_string()))
}

fn action_entry(value: &Value) -> StoreResult<(&str, &serde_json::Map<String, Value>)> {
    let Some(object) = value.as_object() else {
        return Err(StoreError::new(
            400,
            "parse_exception",
            "bulk action line must be a JSON object",
        ));
    };
    if object.len() != 1 {
        return Err(StoreError::new(
            400,
            "parse_exception",
            "bulk action must contain one operation",
        ));
    }
    let (key, value) = object.iter().next().expect("len checked");
    let Some(metadata) = value.as_object() else {
        return Err(StoreError::new(
            400,
            "parse_exception",
            "bulk action metadata must be an object",
        ));
    };
    Ok((key.as_str(), metadata))
}

#[derive(Debug)]
struct MgetItem {
    index: Option<String>,
    id: String,
    source_filter: Option<Value>,
}

fn mget_items(
    body: &Value,
    path_index: Option<&str>,
    query_source_filter: Option<Value>,
) -> Vec<MgetItem> {
    let request_source_filter = body.get("_source").cloned().or(query_source_filter);
    if let Some(ids) = body.get("ids").and_then(Value::as_array) {
        return ids
            .iter()
            .filter_map(json_scalar_string)
            .map(|id| MgetItem {
                index: path_index.map(ToString::to_string),
                id,
                source_filter: request_source_filter.clone(),
            })
            .collect();
    }
    body.get("docs")
        .and_then(Value::as_array)
        .map(|docs| {
            docs.iter()
                .filter_map(|doc| {
                    let id = doc.get("_id").and_then(json_scalar_string)?;
                    Some(MgetItem {
                        index: doc
                            .get("_index")
                            .and_then(Value::as_str)
                            .or(path_index)
                            .map(ToString::to_string),
                        id,
                        source_filter: doc
                            .get("_source")
                            .cloned()
                            .or_else(|| request_source_filter.clone()),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn source_filter_from_query(query: &[(String, String)]) -> Option<Value> {
    let source = query
        .iter()
        .find_map(|(key, value)| (key == "_source").then_some(value.as_str()));
    if let Some(source) = source {
        return match source {
            "true" | "" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            fields => Some(Value::Array(source_filter_fields(fields))),
        };
    }

    let includes = query
        .iter()
        .find_map(|(key, value)| (key == "_source_includes").then(|| source_filter_fields(value)));
    let excludes = query
        .iter()
        .find_map(|(key, value)| (key == "_source_excludes").then(|| source_filter_fields(value)));
    if includes.is_none() && excludes.is_none() {
        return None;
    }

    let mut filter = serde_json::Map::new();
    if let Some(includes) = includes {
        filter.insert("includes".to_string(), Value::Array(includes));
    }
    if let Some(excludes) = excludes {
        filter.insert("excludes".to_string(), Value::Array(excludes));
    }
    Some(Value::Object(filter))
}

fn source_filter_fields(value: &str) -> Vec<Value> {
    value
        .split(',')
        .filter(|field| !field.trim().is_empty())
        .map(|field| Value::String(field.trim().to_string()))
        .collect()
}

fn mget_doc_response(db: &Database, item: MgetItem) -> Value {
    let Some(index) = item.index else {
        return json!({
            "_id": item.id,
            "found": false,
            "error": {
                "type": "index_missing_exception",
                "reason": "_mget item requires _index or a path index"
            }
        });
    };
    let Some(index_name) = db.resolve_index(&index) else {
        return json!({ "_index": index, "_id": item.id, "found": false });
    };
    match db
        .indexes
        .get(&index_name)
        .and_then(|index| index.documents.get(&item.id))
    {
        Some(doc) => {
            let mut response = json!({
                "_index": index_name,
                "_id": item.id,
                "_version": doc.version,
                "_seq_no": doc.seq_no,
                "_primary_term": doc.primary_term,
                "found": true
            });
            if item.source_filter.as_ref() != Some(&Value::Bool(false)) {
                response["_source"] = search_engine::evaluator::filter_source(
                    &doc.source,
                    item.source_filter.as_ref(),
                );
            }
            response
        }
        None => json!({ "_index": index_name, "_id": item.id, "found": false }),
    }
}

fn path_indices(path_index: Option<&str>, suffix: &str) -> Vec<String> {
    path_index
        .filter(|index| *index != suffix)
        .map(|index| {
            index
                .split(',')
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn scroll_id_from_request(request: &Request) -> Option<String> {
    request
        .query_value("scroll_id")
        .map(ToString::to_string)
        .or_else(|| {
            request
                .body_json()
                .ok()
                .and_then(|body| {
                    body.get("scroll_id")
                        .or_else(|| body.get("scrollId"))
                        .cloned()
                })
                .and_then(|value| match value {
                    Value::String(value) => Some(value),
                    Value::Array(values) => values
                        .first()
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    _ => None,
                })
        })
}

fn scroll_ids_from_request(request: &Request) -> Vec<String> {
    let mut ids = request
        .query
        .iter()
        .filter_map(|(key, value)| (key == "scroll_id").then_some(value.clone()))
        .collect::<Vec<_>>();
    if let Ok(body) = request.body_json() {
        if let Some(value) = body.get("scroll_id").or_else(|| body.get("scrollId")) {
            match value {
                Value::String(value) => ids.push(value.clone()),
                Value::Array(values) => ids.extend(
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string),
                ),
                _ => {}
            }
        }
    }
    ids
}

fn comma_query_values(value: Option<&str>) -> Option<Vec<String>> {
    value.map(|value| {
        value
            .split(',')
            .filter(|part| !part.trim().is_empty())
            .map(|part| part.trim().to_string())
            .collect::<Vec<_>>()
    })
}

fn bool_query(value: Option<&str>) -> bool {
    matches!(value, Some("true") | Some(""))
}

fn msearch_indices(header: &Value, path_index: Option<&str>) -> Vec<String> {
    if let Some(index) = header.get("index").and_then(Value::as_str) {
        return index.split(',').map(ToString::to_string).collect();
    }
    if let Some(indices) = header.get("index").and_then(Value::as_array) {
        return indices
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(indices) = header.get("indices").and_then(Value::as_array) {
        return indices
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    path_indices(path_index, "_msearch")
}

fn numeric_param(body: &Value, query: &[(String, String)], key: &str, default: usize) -> usize {
    query
        .iter()
        .find_map(|(name, value)| (name == key).then(|| value.parse().ok()).flatten())
        .or_else(|| {
            body.get(key)
                .and_then(Value::as_u64)
                .map(|value| value as usize)
        })
        .unwrap_or(default)
}

fn search_validation_error_type(reason: &str) -> &'static str {
    if reason.contains("from + size") {
        "illegal_argument_exception"
    } else {
        "x_content_parse_exception"
    }
}

fn validate_search_indices(db: &Database, indices: &[String]) -> StoreResult<()> {
    for index in indices {
        if index == "_all" || index == "*" || index.contains('*') {
            continue;
        }
        if db.resolve_index(index).is_none() {
            return Err(StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{index}]"),
            ));
        }
    }
    Ok(())
}

fn query_value(query: &[(String, String)]) -> Value {
    let mut object = serde_json::Map::new();
    for (key, value) in query {
        match object.get_mut(key) {
            Some(Value::Array(values)) => values.push(Value::String(value.clone())),
            Some(existing) => {
                let first = existing.take();
                *existing = Value::Array(vec![first, Value::String(value.clone())]);
            }
            None => {
                object.insert(key.clone(), Value::String(value.clone()));
            }
        }
    }
    Value::Object(object)
}

fn redact_secret_values(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    if secret_like_key(&key) {
                        (key, Value::String("[REDACTED]".to_string()))
                    } else {
                        (key, redact_secret_values(value))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_secret_values).collect())
        }
        other => other,
    }
}

fn secret_like_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("password")
        || key.contains("passwd")
        || key.contains("token")
        || key.contains("secret")
        || key.contains("authorization")
        || key == "api_key"
        || key == "apikey"
}

fn agent_catalog_context(db: &Database, request: &Request, api_name: &str) -> Value {
    const DOCUMENT_LIMIT_PER_INDEX: usize = 100;

    let mut requested = if api_name == "agent.read" {
        Vec::new()
    } else {
        requested_indices(request)
    };
    requested.sort();
    requested.dedup();

    if requested.is_empty() {
        let indexes = db
            .indexes
            .values()
            .map(|index| {
                json!({
                    "name": index.name,
                    "document_count": index.documents.len(),
                    "aliases": index.aliases,
                })
            })
            .collect::<Vec<_>>();
        return json!({
            "scope": "metadata_only",
            "reason": "request did not identify target indices",
            "indexes": indexes,
            "documents_included": 0,
            "documents_omitted": db.document_count()
        });
    }

    let mut indexes = serde_json::Map::new();
    let mut missing = Vec::new();
    let mut documents_included = 0usize;
    let mut documents_omitted = 0usize;

    for requested_name in requested {
        let Some(index_name) = db.resolve_index(&requested_name) else {
            missing.push(requested_name);
            continue;
        };
        let Some(index) = db.indexes.get(&index_name) else {
            missing.push(requested_name);
            continue;
        };
        let documents = index
            .documents
            .values()
            .take(DOCUMENT_LIMIT_PER_INDEX)
            .map(|doc| {
                documents_included += 1;
                json!({
                    "_id": doc.id,
                    "_version": doc.version,
                    "_seq_no": doc.seq_no,
                    "_source": doc.source
                })
            })
            .collect::<Vec<_>>();
        documents_omitted += index
            .documents
            .len()
            .saturating_sub(DOCUMENT_LIMIT_PER_INDEX);
        indexes.insert(
            index_name.clone(),
            json!({
                "settings": index.settings,
                "mappings": index.mappings,
                "aliases": index.aliases,
                "document_count": index.documents.len(),
                "documents": documents
            }),
        );
    }

    json!({
        "scope": "targeted",
        "requested_indices": missing.iter().chain(indexes.keys()).cloned().collect::<Vec<_>>(),
        "missing_indices": missing,
        "indexes": indexes,
        "documents_included": documents_included,
        "documents_omitted": documents_omitted
    })
}

fn requested_indices(request: &Request) -> Vec<String> {
    let mut indices = Vec::new();
    let parts = segments(&request.path);
    if let Some(first) = parts.first() {
        if !first.starts_with('_') {
            indices.extend(first.split(',').map(ToString::to_string));
        }
    }
    for (key, value) in &request.query {
        if matches!(key.as_str(), "index" | "indices") {
            indices.extend(value.split(',').map(ToString::to_string));
        }
    }
    indices.retain(|index| !index.trim().is_empty());
    indices
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

fn strict_guard(state: &AppState, route: &api_spec::RouteMatch) -> Option<Response> {
    if !state.config.strict_compatibility {
        return None;
    }
    if route.tier == Tier::Implemented
        || state
            .config
            .strict_allowlist
            .iter()
            .any(|allowed| allowed == route.api_name || allowed == "*")
    {
        return None;
    }
    Some(open_search_error(
        501,
        "opensearch_lite_strict_compatibility_exception",
        format!(
            "route [{}] is tier [{}] and strict compatibility mode is enabled",
            route.api_name,
            route.tier.as_str()
        ),
        Some("Use an implemented route, add this route to --strict-allowlist for this run, or test against real OpenSearch."),
    ))
}

fn unsupported(api_name: &str) -> Response {
    open_search_error(
        501,
        "opensearch_lite_unsupported_api_exception",
        format!("OpenSearch Lite does not implement [{api_name}] yet"),
        Some("Use an implemented API, simplify the request, use a mocked local no-op API where safe, or configure agent fallback for eligible routes."),
    )
}

fn write_fallback_disabled(api_name: &str) -> Response {
    open_search_error(
        501,
        "opensearch_lite_agent_write_fallback_disabled_exception",
        format!("OpenSearch Lite can only handle [{api_name}] through write-enabled agent fallback, which is not enabled"),
        Some("Enable write fallback only for trusted local development, or use full OpenSearch locally, server-hosted OpenSearch, or cloud-hosted OpenSearch for this API family."),
    )
}

fn parse_error(error: String) -> Response {
    open_search_error(
        400,
        "parse_exception",
        error,
        Some("Check JSON/NDJSON syntax and retry."),
    )
}

fn store_error(error: StoreError) -> Response {
    open_search_error(error.status, error.error_type, error.reason, None)
}

async fn run_store<T>(
    store: Store,
    f: impl FnOnce(Store) -> StoreResult<T> + Send + 'static,
) -> StoreResult<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || f(store))
        .await
        .map_err(|error| StoreError::new(500, "task_join_exception", error.to_string()))?
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use bytes::Bytes;
    use http::{HeaderMap, Method, Uri};
    use serde_json::json;

    use super::*;
    use crate::storage::{IndexMetadata, StoredDocument};

    fn request(path: &str) -> Request {
        Request::from_parts(
            Method::GET,
            path.parse::<Uri>().unwrap(),
            HeaderMap::new(),
            Bytes::new(),
        )
    }

    fn database_with_private_doc() -> Database {
        let mut documents = BTreeMap::new();
        documents.insert(
            "1".to_string(),
            StoredDocument {
                id: "1".to_string(),
                source: json!({ "secret": "local-only" }),
                version: 1,
                seq_no: 1,
                primary_term: 1,
            },
        );
        let mut indexes = BTreeMap::new();
        indexes.insert(
            "private".to_string(),
            IndexMetadata {
                name: "private".to_string(),
                settings: json!({}),
                mappings: json!({}),
                aliases: BTreeSet::new(),
                documents,
                tombstones: BTreeMap::new(),
                store_size_bytes: 100,
            },
        );
        Database {
            indexes,
            templates: BTreeMap::new(),
            registries: BTreeMap::new(),
            aliases: BTreeMap::new(),
            seq_no: 1,
        }
    }

    #[test]
    fn unknown_agent_fallback_ignores_attacker_supplied_index_query() {
        let db = database_with_private_doc();
        let catalog = agent_catalog_context(
            &db,
            &request("/_plugins/_unknown_read?index=private"),
            "agent.read",
        );

        assert_eq!(catalog["scope"], "metadata_only");
        assert_eq!(catalog["documents_included"], 0);
        assert_eq!(catalog["documents_omitted"], 1);
    }

    #[test]
    fn known_read_fallback_can_scope_to_validated_target_index() {
        let db = database_with_private_doc();
        let catalog = agent_catalog_context(&db, &request("/private/_count"), "count");

        assert_eq!(catalog["scope"], "targeted");
        assert_eq!(catalog["documents_included"], 1);
        assert_eq!(
            catalog["indexes"]["private"]["documents"][0]["_source"]["secret"],
            "local-only"
        );
    }
}
