pub mod aliases;
pub mod bulk;
pub mod cat;
pub mod cluster;
pub mod documents;
pub mod indices;
pub mod search;
pub mod templates;

use http::Method;
use serde_json::{json, Value};

use crate::{
    agent::{validation::failure_response, AgentRequestContext},
    api_spec::{self, Tier},
    http::request::Request,
    responses::{acknowledged, best_effort, info, logging, open_search_error, Response},
    search::{self as search_engine, dsl::SearchRequest},
    server::AppState,
    storage::{
        mutation_log::Mutation, Database, Store, StoreError, StoreResult, WriteOperation,
        WriteOutcome,
    },
};

pub async fn handle_request(state: AppState, request: Request) -> Response {
    let route = api_spec::classify(&request.method, &request.path);
    if let Some(response) = strict_guard(&state, &route) {
        return response;
    }

    match route.tier {
        Tier::Implemented => handle_implemented(state, request, route.api_name).await,
        Tier::BestEffort => handle_best_effort(state, request, route.api_name),
        Tier::AgentRead => handle_agent_read(state, request, route.api_name).await,
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
    if request.path == "/_bulk" || parts.get(1) == Some(&"_bulk") {
        let path_index = if request.path == "/_bulk" {
            None
        } else {
            parts.first().copied()
        };
        return handle_bulk(&state, &request, path_index).await;
    }
    if request.path == "/_search" || parts.get(1) == Some(&"_search") {
        return handle_search(&state, &request, parts.first().copied());
    }
    if request.path == "/_count" || parts.get(1) == Some(&"_count") {
        return handle_count(&state, &request, parts.first().copied());
    }
    if request.path == "/_mget" || parts.get(1) == Some(&"_mget") {
        return handle_mget(&state, &request, parts.first().copied());
    }
    if request.path == "/_msearch" || parts.get(1) == Some(&"_msearch") {
        return handle_msearch(&state, &request, parts.first().copied());
    }
    if request.path == "/_refresh" || parts.get(1) == Some(&"_refresh") {
        return handle_refresh(&state, parts.first().copied());
    }
    if parts.first() == Some(&"_mapping") || parts.get(1) == Some(&"_mapping") {
        return handle_mapping(&state, &request, parts.first().copied()).await;
    }
    if parts.first() == Some(&"_settings") || parts.get(1) == Some(&"_settings") {
        return handle_settings(&state, &request, parts.first().copied()).await;
    }
    if parts.first() == Some(&"_index_template") {
        return handle_template(&state, &request, parts.get(1).copied()).await;
    }
    if parts.first() == Some(&"_alias")
        || parts.first() == Some(&"_aliases")
        || parts.get(1) == Some(&"_alias")
        || parts.get(1) == Some(&"_aliases")
    {
        return handle_alias(&state, &request, &parts).await;
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
    match request.path.as_str() {
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
        path if path.starts_with("/_nodes") => Response::json(
            200,
            json!({
                "cluster_name": "opensearch-lite",
                "nodes": {}
            }),
        )
        .compatibility_signal(api_name, "best_effort"),
        path if path.starts_with("/_cat/indices") => cat_indices(&state, api_name),
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
    }
}

async fn handle_agent_read(state: AppState, request: Request, api_name: &str) -> Response {
    let body = if request.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&request.body).unwrap_or_else(|_| {
            json!({
                "unparsed_body": String::from_utf8_lossy(&request.body).to_string()
            })
        })
    };
    let query = query_value(&request.query);
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
    };
    match state.agent.complete(context).await {
        Ok(response) => response,
        Err(error) => failure_response(error),
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
                        "index_template": template.raw
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
                let index = index.to_string();
                let alias = alias.to_string();
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
        (Method::POST, ["_aliases"]) => handle_alias_actions(state, request).await,
        (Method::HEAD, ["_alias", alias]) => alias_exists_response(state, None, alias),
        (Method::HEAD, [index, "_alias", alias]) => {
            alias_exists_response(state, Some(index), alias)
        }
        (Method::HEAD, [index, "_aliases", alias]) => {
            alias_exists_response(state, Some(index), alias)
        }
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
    let actions = body
        .get("actions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for action in actions {
        let Some(action) = action.as_object() else {
            return parse_error("alias action must be an object".to_string());
        };
        let Some((kind, meta)) = action.iter().next() else {
            return parse_error("alias action must contain add or remove".to_string());
        };
        let Some(meta) = meta.as_object() else {
            return parse_error("alias action metadata must be an object".to_string());
        };
        let index = meta
            .get("index")
            .or_else(|| meta.get("indices").and_then(Value::as_array)?.first())
            .and_then(Value::as_str)
            .unwrap_or("");
        let alias = meta
            .get("alias")
            .or_else(|| meta.get("aliases").and_then(Value::as_array)?.first())
            .and_then(Value::as_str)
            .unwrap_or("");
        if index.is_empty() || alias.is_empty() {
            return parse_error("alias action requires index and alias".to_string());
        }
        let index = index.to_string();
        let alias = alias.to_string();
        let result = match kind.as_str() {
            "add" => {
                let raw = meta.clone();
                run_store(state.store.clone(), move |store| {
                    store.put_alias(&index, &alias, Value::Object(raw))
                })
                .await
            }
            "remove" => {
                run_store(state.store.clone(), move |store| {
                    store.delete_alias(&index, &alias)
                })
                .await
            }
            other => Err(StoreError::new(
                400,
                "illegal_argument_exception",
                format!("unsupported alias action [{other}]"),
            )),
        };
        if let Err(error) = result {
            return store_error(error);
        }
    }
    acknowledged(true)
}

fn alias_response(state: &AppState, index: Option<&str>, alias: Option<&str>) -> Response {
    let db = state.store.database();
    if let Some(index) = index {
        if db.resolve_index(index).is_none() {
            return store_error(StoreError::new(
                404,
                "index_not_found_exception",
                format!("no such index [{index}]"),
            ));
        }
    }
    let mut output = serde_json::Map::new();
    for (alias_name, meta) in &db.aliases {
        if alias.map(|alias| alias != alias_name).unwrap_or(false) {
            continue;
        }
        if index.map(|index| index != meta.index).unwrap_or(false) {
            continue;
        }
        let entry = output
            .entry(meta.index.clone())
            .or_insert_with(|| json!({ "aliases": {} }));
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
    let id = parts.get(2).copied();
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
            let index = index.to_string();
            let id = id.to_string();
            let index_for_store = index.clone();
            let id_for_store = id.clone();
            match run_store(state.store.clone(), move |store| {
                store.delete_document(&index_for_store, &id_for_store)
            })
            .await
            {
                Ok(found) => Response::json(
                    if found { 200 } else { 404 },
                    json!({
                        "_index": index,
                        "_id": id,
                        "result": if found { "deleted" } else { "not_found" },
                        "_shards": { "total": 1, "successful": 1, "failed": 0 }
                    }),
                ),
                Err(error) => store_error(error),
            }
        }
        (Method::POST, "_update") => {
            let Some(id) = id else {
                return parse_error("document id is required".to_string());
            };
            match request.body_json() {
                Ok(body) if body.get("script").is_some() => unsupported("update.script"),
                Ok(body) => {
                    let doc = body.get("doc").cloned().unwrap_or_else(|| json!({}));
                    let upsert = body
                        .get("doc_as_upsert")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let index = index.to_string();
                    let id = id.to_string();
                    let index_for_store = index.clone();
                    let id_for_store = id.clone();
                    match run_store(state.store.clone(), move |store| {
                        store.update_document(&index_for_store, &id_for_store, doc, upsert)
                    })
                    .await
                    {
                        Ok(doc) => doc_write_response(&index, &id, &doc, 200, "updated"),
                        Err(error) => store_error(error),
                    }
                }
                Err(error) => parse_error(error),
            }
        }
        _ => unsupported("document"),
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
                    let doc = body.get("doc").cloned().unwrap_or_else(|| json!({}));
                    let upsert = body
                        .get("doc_as_upsert")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    plans.push(BulkPlan::Store(BulkStorePlan {
                        action: action.to_string(),
                        index: index.clone(),
                        id: id.clone(),
                        operation: WriteOperation::UpdateDocument {
                            index,
                            id,
                            doc,
                            doc_as_upsert: upsert,
                        },
                    }));
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
                        (200, "updated")
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
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
    let from = numeric_param(&body, &request.query, "from", 0);
    let size = numeric_param(&body, &request.query, "size", 10);
    if from.saturating_add(size) > state.config.max_result_window {
        return open_search_error(
            400,
            "illegal_argument_exception",
            "from + size exceeds max result window",
            Some("Use a smaller page for OpenSearch Lite or raise --max-result-window."),
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
                size,
            },
        )
    });
    match search_result {
        Ok(Ok(body)) => Response::json(200, body),
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

fn handle_count(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
    let body = match request.body_json() {
        Ok(body) => body,
        Err(error) => return parse_error(error),
    };
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
    match state
        .store
        .read_database(|db| validate_search_indices(db, &indices))
    {
        Ok(Ok(())) => Response::json(
            200,
            json!({
                "_shards": {
                    "total": 1,
                    "successful": 1,
                    "failed": 0
                }
            }),
        ),
        Ok(Err(error)) | Err(error) => store_error(error),
    }
}

fn handle_msearch(state: &AppState, request: &Request, path_index: Option<&str>) -> Response {
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
        let response = state.store.read_database(|db| {
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

fn cat_indices(state: &AppState, api_name: &str) -> Response {
    let db = state.store.database();
    let rows = db
        .indexes
        .values()
        .map(|index| {
            json!({
                "health": "green",
                "status": "open",
                "index": index.name,
                "uuid": format!("opensearch-lite-{}", index.name),
                "pri": "1",
                "rep": "0",
                "docs.count": index.documents.len().to_string(),
                "docs.deleted": index.tombstones.len().to_string(),
                "store.size": "0b",
                "pri.store.size": "0b"
            })
        })
        .collect::<Vec<_>>();
    Response::json(200, Value::Array(rows)).compatibility_signal(api_name, "best_effort")
}

fn doc_write_response(
    index: &str,
    id: &str,
    doc: &crate::storage::StoredDocument,
    status: u16,
    result: &str,
) -> Response {
    Response::json(
        status,
        json!({
            "_index": index,
            "_id": id,
            "_version": doc.version,
            "result": result,
            "_shards": { "total": 1, "successful": 1, "failed": 0 },
            "_seq_no": doc.seq_no,
            "_primary_term": doc.primary_term
        }),
    )
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
            .filter_map(Value::as_str)
            .map(|id| MgetItem {
                index: path_index.map(ToString::to_string),
                id: id.to_string(),
                source_filter: request_source_filter.clone(),
            })
            .collect();
    }
    body.get("docs")
        .and_then(Value::as_array)
        .map(|docs| {
            docs.iter()
                .filter_map(|doc| {
                    let id = doc.get("_id").and_then(Value::as_str)?;
                    Some(MgetItem {
                        index: doc
                            .get("_index")
                            .and_then(Value::as_str)
                            .or(path_index)
                            .map(ToString::to_string),
                        id: id.to_string(),
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
        Some("Use an implemented API, simplify the request, or configure read-only agent fallback for eligible read routes."),
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
            },
        );
        Database {
            indexes,
            templates: BTreeMap::new(),
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
