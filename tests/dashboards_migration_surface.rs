mod support;

use http::Method;
use serde_json::{json, Value};
use support::{call, durable_state, ephemeral_state};

#[tokio::test]
async fn scroll_and_clear_scroll_page_saved_object_reads() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 1), ("2", 2), ("3", 3)] {
        call(
            &state,
            Method::PUT,
            &format!("/saved/_doc/{id}"),
            json!({ "rank": rank, "type": "dashboard" }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/saved/_search?scroll=1m",
        json!({
            "size": 2,
            "sort": [{ "rank": { "order": "asc" } }]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let body = first.body.unwrap();
    let scroll_id = body["_scroll_id"].as_str().unwrap().to_string();
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 2);
    assert_eq!(body["hits"]["hits"][0]["_id"], "1");
    assert_eq!(body["hits"]["hits"][1]["_id"], "2");

    let second = call(
        &state,
        Method::POST,
        "/_search/scroll",
        json!({ "scroll": "1m", "scroll_id": scroll_id }),
    )
    .await;
    assert_eq!(second.status, 200);
    let body = second.body.unwrap();
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"]["hits"][0]["_id"], "3");

    let clear = call(
        &state,
        Method::DELETE,
        "/_search/scroll",
        json!({ "scroll_id": scroll_id }),
    )
    .await;
    assert_eq!(clear.status, 200);
    assert_eq!(clear.body.unwrap()["num_freed"], 1);

    let missing = call(
        &state,
        Method::POST,
        "/_search/scroll",
        json!({ "scroll_id": scroll_id }),
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "search_context_missing_exception"
    );
}

#[tokio::test]
async fn path_form_scroll_reads_scroll_id() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 1), ("2", 2), ("3", 3)] {
        call(
            &state,
            Method::PUT,
            &format!("/saved/_doc/{id}"),
            json!({ "rank": rank, "type": "dashboard" }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/saved/_search?scroll=1m",
        json!({
            "size": 1,
            "sort": [{ "rank": { "order": "asc" } }]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let scroll_id = first.body.unwrap()["_scroll_id"]
        .as_str()
        .unwrap()
        .to_string();
    let path_scroll_id = scroll_id.replace(':', "%3A");

    let second = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(second.status, 200);
    assert_eq!(second.body.unwrap()["hits"]["hits"][0]["_id"], "2");

    let third = call(
        &state,
        Method::POST,
        &format!("/_search/scroll/{path_scroll_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(third.status, 200);
    assert_eq!(third.body.unwrap()["hits"]["hits"][0]["_id"], "3");
}

#[tokio::test]
async fn path_form_clear_scroll_decodes_scroll_id() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 1), ("2", 2), ("3", 3)] {
        call(
            &state,
            Method::PUT,
            &format!("/saved/_doc/{id}"),
            json!({ "rank": rank, "type": "dashboard" }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/saved/_search?scroll=1m",
        json!({
            "size": 1,
            "sort": [{ "rank": { "order": "asc" } }]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let scroll_id = first.body.unwrap()["_scroll_id"]
        .as_str()
        .unwrap()
        .to_string();
    let path_scroll_id = scroll_id.replace(':', "%3A");

    let clear = call(
        &state,
        Method::DELETE,
        &format!("/_search/scroll/{path_scroll_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(clear.status, 200);
    assert_eq!(clear.body.unwrap()["num_freed"], 1);

    let missing = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "search_context_missing_exception"
    );
}

#[tokio::test]
async fn exhausted_scroll_id_returns_one_empty_page_for_dashboards_migrations() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "rank": 1, "type": "index-pattern" }),
    )
    .await;

    let first = call(
        &state,
        Method::POST,
        "/saved/_search?scroll=15m",
        json!({ "size": 1000 }),
    )
    .await;
    assert_eq!(first.status, 200);
    let scroll_id = first.body.unwrap()["_scroll_id"]
        .as_str()
        .unwrap()
        .to_string();
    let path_scroll_id = scroll_id.replace(':', "%3A");

    let exhausted = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}?scroll=15m"),
        Value::Null,
    )
    .await;
    assert_eq!(exhausted.status, 200);
    assert_eq!(
        exhausted.body.unwrap()["hits"]["hits"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let missing = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}?scroll=15m"),
        Value::Null,
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "search_context_missing_exception"
    );
}

#[tokio::test]
async fn multi_page_scroll_returns_terminal_empty_page_before_missing() {
    let state = ephemeral_state();
    for (id, rank) in [("1", 1), ("2", 2)] {
        call(
            &state,
            Method::PUT,
            &format!("/saved/_doc/{id}"),
            json!({ "rank": rank, "type": "index-pattern" }),
        )
        .await;
    }

    let first = call(
        &state,
        Method::POST,
        "/saved/_search?scroll=15m",
        json!({
            "size": 1,
            "sort": [{ "rank": { "order": "asc" } }]
        }),
    )
    .await;
    assert_eq!(first.status, 200);
    let first_body = first.body.unwrap();
    assert_eq!(first_body["hits"]["hits"][0]["_id"], "1");
    let scroll_id = first_body["_scroll_id"].as_str().unwrap().to_string();
    let path_scroll_id = scroll_id.replace(':', "%3A");

    let final_hit = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}?scroll=15m"),
        Value::Null,
    )
    .await;
    assert_eq!(final_hit.status, 200);
    assert_eq!(final_hit.body.unwrap()["hits"]["hits"][0]["_id"], "2");

    let terminal_empty = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}?scroll=15m"),
        Value::Null,
    )
    .await;
    assert_eq!(terminal_empty.status, 200);
    assert_eq!(
        terminal_empty.body.unwrap()["hits"]["hits"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let missing = call(
        &state,
        Method::GET,
        &format!("/_search/scroll/{path_scroll_id}?scroll=15m"),
        Value::Null,
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.unwrap()["error"]["type"],
        "search_context_missing_exception"
    );
}

#[tokio::test]
async fn reindex_records_completed_task_for_dashboards_polling() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/.opensearch_dashboards/_doc/1",
        json!({ "type": "dashboard", "title": "Sales" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/.opensearch_dashboards/_doc/2",
        json!({ "type": "visualization", "title": "Revenue" }),
    )
    .await;

    let reindex = call(
        &state,
        Method::POST,
        "/_reindex?wait_for_completion=false&refresh=true",
        json!({
            "source": {
                "index": ".opensearch_dashboards",
                "size": 10
            },
            "dest": {
                "index": ".opensearch_dashboards_1"
            },
            "script": {
                "source": "ctx._id = ctx._source.type + ':' + ctx._id",
                "lang": "painless"
            }
        }),
    )
    .await;
    assert_eq!(reindex.status, 200);
    let task = reindex.body.unwrap()["task"].as_str().unwrap().to_string();

    let task_path_id = task.replace(':', "%3A");
    let task_response = call(
        &state,
        Method::GET,
        &format!("/_tasks/{task_path_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(task_response.status, 200);
    let body = task_response.body.unwrap();
    assert_eq!(body["completed"], true);
    assert_eq!(body["response"]["total"], 2);
    assert_eq!(body["response"]["created"], 2);

    let dashboard = call(
        &state,
        Method::GET,
        "/.opensearch_dashboards_1/_doc/dashboard:1",
        Value::Null,
    )
    .await;
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body.unwrap()["_source"]["title"], "Sales");
}

#[tokio::test]
async fn older_opensearch_dashboards_index_migration_survives_durable_restart() {
    let temp = tempfile::tempdir().unwrap();
    let state = durable_state(temp.path());
    call(
        &state,
        Method::PUT,
        "/.opensearch_dashboards/_doc/1",
        json!({ "type": "dashboard", "title": "Sales", "references": [] }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/.opensearch_dashboards/_doc/2",
        json!({ "type": "visualization", "title": "Revenue", "references": [] }),
    )
    .await;

    let reindex = call(
        &state,
        Method::POST,
        "/_reindex?wait_for_completion=false&refresh=true",
        json!({
            "source": {
                "index": ".opensearch_dashboards",
                "size": 10
            },
            "dest": {
                "index": ".opensearch_dashboards_1"
            },
            "script": {
                "source": "ctx._id = ctx._source.type + ':' + ctx._id",
                "lang": "painless"
            }
        }),
    )
    .await;
    assert_eq!(reindex.status, 200);
    assert!(reindex.body.unwrap()["task"].as_str().is_some());

    let alias = call(
        &state,
        Method::POST,
        "/_aliases",
        json!({
            "actions": [
                { "remove_index": { "index": ".opensearch_dashboards" } },
                { "add": { "index": ".opensearch_dashboards_1", "alias": ".opensearch_dashboards" } }
            ]
        }),
    )
    .await;
    assert_eq!(alias.status, 200);

    drop(state);
    let replayed = durable_state(temp.path());
    let dashboard = call(
        &replayed,
        Method::GET,
        "/.opensearch_dashboards/_doc/dashboard%3A1",
        Value::Null,
    )
    .await;
    assert_eq!(dashboard.status, 200);
    let body = dashboard.body.unwrap();
    assert_eq!(body["_id"], "dashboard:1");
    assert_eq!(body["_source"]["title"], "Sales");

    let count = call(
        &replayed,
        Method::POST,
        "/.opensearch_dashboards/_count",
        json!({ "query": { "match_all": {} } }),
    )
    .await;
    assert_eq!(count.status, 200);
    assert_eq!(count.body.unwrap()["count"], 2);
}

#[tokio::test]
async fn reindex_returns_synchronous_response_by_default() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/source/_doc/1",
        json!({ "type": "dashboard", "title": "Sales" }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/_reindex",
        json!({
            "source": { "index": "source" },
            "dest": { "index": "dest" }
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert!(body.get("task").is_none());
    assert_eq!(body["total"], 1);
    assert_eq!(body["created"], 1);

    let copied = call(&state, Method::GET, "/dest/_doc/1", Value::Null).await;
    assert_eq!(copied.status, 200);
    assert_eq!(copied.body.unwrap()["_source"]["title"], "Sales");
}

#[tokio::test]
async fn reindex_create_op_type_reports_conflicts_without_overwrite() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/source/_doc/1",
        json!({ "title": "source" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/dest/_doc/1",
        json!({ "title": "existing" }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/_reindex",
        json!({
            "source": { "index": "source" },
            "dest": { "index": "dest", "op_type": "create" }
        }),
    )
    .await;
    assert_eq!(response.status, 409);
    assert_eq!(
        response.body.unwrap()["error"]["type"],
        "version_conflict_engine_exception"
    );

    let existing = call(&state, Method::GET, "/dest/_doc/1", Value::Null).await;
    assert_eq!(existing.status, 200);
    assert_eq!(existing.body.unwrap()["_source"]["title"], "existing");
}

#[tokio::test]
async fn reindex_create_op_type_can_proceed_past_conflicts() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/source/_doc/1",
        json!({ "title": "one" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/source/_doc/2",
        json!({ "title": "two" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/dest/_doc/1",
        json!({ "title": "existing" }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/_reindex?conflicts=proceed",
        json!({
            "source": { "index": "source" },
            "dest": { "index": "dest", "op_type": "create" }
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["total"], 2);
    assert_eq!(body["created"], 1);
    assert_eq!(body["version_conflicts"], 1);

    let existing = call(&state, Method::GET, "/dest/_doc/1", Value::Null).await;
    assert_eq!(existing.body.unwrap()["_source"]["title"], "existing");
    let created = call(&state, Method::GET, "/dest/_doc/2", Value::Null).await;
    assert_eq!(created.status, 200);
    assert_eq!(created.body.unwrap()["_source"]["title"], "two");
}

#[tokio::test]
async fn delete_by_query_uses_shared_query_evaluator_and_mutates_matches() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "type": "dashboard", "title": "Sales" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/saved/_doc/2",
        json!({ "type": "visualization", "title": "Revenue" }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/saved/_delete_by_query?conflicts=proceed&refresh=true",
        json!({
            "query": {
                "term": {
                    "type": "dashboard"
                }
            }
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["deleted"], 1);

    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/1", Value::Null)
            .await
            .status,
        404
    );
    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/2", Value::Null)
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn delete_by_query_requires_explicit_query_and_exact_route_shape() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "type": "dashboard" }),
    )
    .await;

    let missing_query = call(&state, Method::POST, "/saved/_delete_by_query", json!({})).await;
    assert_eq!(missing_query.status, 400);
    assert_eq!(
        missing_query.body.unwrap()["error"]["type"],
        "action_request_validation_exception"
    );

    for path in ["/_delete_by_query", "/saved/_delete_by_query/extra"] {
        let response = call(
            &state,
            Method::POST,
            path,
            json!({ "query": { "match_all": {} } }),
        )
        .await;
        assert_eq!(response.status, 501, "{path}");
    }

    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/1", Value::Null)
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn update_by_query_supports_saved_object_namespace_removal_script() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "type": "dashboard", "namespaces": ["default", "space-a"] }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/saved/_doc/2",
        json!({ "type": "visualization" }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/saved/_doc/3",
        json!({ "type": "search", "namespaces": ["space-a"] }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/saved/_update_by_query?conflicts=proceed&refresh=true",
        json!({
            "script": {
                "source": "
                  if (!ctx._source.containsKey('namespaces')) {
                    ctx.op = \"delete\";
                  } else {
                    ctx._source['namespaces'].removeAll(Collections.singleton(params['namespace']));
                    if (ctx._source['namespaces'].empty) {
                      ctx.op = \"delete\";
                    }
                  }
                ",
                "lang": "painless",
                "params": { "namespace": "space-a" }
            },
            "query": {
                "match_all": {}
            }
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["total"], 3);
    assert_eq!(body["updated"], 1);
    assert_eq!(body["deleted"], 2);

    let kept = call(&state, Method::GET, "/saved/_doc/1", Value::Null).await;
    assert_eq!(kept.status, 200);
    assert_eq!(
        kept.body.unwrap()["_source"]["namespaces"],
        json!(["default"])
    );
    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/2", Value::Null)
            .await
            .status,
        404
    );
    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/3", Value::Null)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn update_by_query_supports_saved_object_workspace_removal_script() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "type": "dashboard", "workspaces": ["default", "workspace-a"] }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/saved/_doc/2",
        json!({ "type": "search", "workspaces": ["workspace-a"] }),
    )
    .await;
    call(
        &state,
        Method::PUT,
        "/saved/_doc/3",
        json!({ "type": "visualization", "workspaces": ["other"] }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/saved/_update_by_query?conflicts=proceed&refresh=true",
        json!({
            "script": {
                "source": "
                  if (!ctx._source.containsKey('workspaces')) {
                    ctx.op = \"delete\";
                  } else {
                    ctx._source['workspaces'].removeAll(Collections.singleton(params['workspace']));
                    if (ctx._source['workspaces'].empty) {
                      ctx.op = \"delete\";
                    }
                  }
                ",
                "lang": "painless",
                "params": { "workspace": "workspace-a" }
            },
            "query": {
                "term": {
                    "workspaces": "workspace-a"
                }
            }
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    let body = response.body.unwrap();
    assert_eq!(body["total"], 2);
    assert_eq!(body["updated"], 1);
    assert_eq!(body["deleted"], 1);

    let kept = call(&state, Method::GET, "/saved/_doc/1", Value::Null).await;
    assert_eq!(kept.status, 200);
    assert_eq!(
        kept.body.unwrap()["_source"]["workspaces"],
        json!(["default"])
    );
    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/2", Value::Null)
            .await
            .status,
        404
    );
    assert_eq!(
        call(&state, Method::GET, "/saved/_doc/3", Value::Null)
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn update_by_query_rejects_unknown_scripts_without_mutation() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "type": "dashboard", "namespaces": ["default"] }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/saved/_update_by_query",
        json!({
            "script": {
                "source": "ctx._source.title = 'changed'",
                "lang": "painless"
            },
            "query": {
                "match_all": {}
            }
        }),
    )
    .await;
    assert_eq!(response.status, 400);
    assert_eq!(response.body.unwrap()["error"]["type"], "script_exception");

    let unchanged = call(&state, Method::GET, "/saved/_doc/1", Value::Null).await;
    assert_eq!(unchanged.status, 200);
    assert!(unchanged.body.unwrap()["_source"].get("title").is_none());
}

#[tokio::test]
async fn update_by_query_rejects_near_miss_scripts_before_matching() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/saved/_doc/1",
        json!({ "type": "dashboard", "namespaces": ["default"] }),
    )
    .await;

    let response = call(
        &state,
        Method::POST,
        "/saved/_update_by_query",
        json!({
            "script": {
                "source": "
                  // params['namespace'] and namespaces are mentioned, but this is not a removal migration.
                  ctx._source['namespaces'].add(params['namespace']);
                ",
                "lang": "painless",
                "params": { "namespace": "space-a" }
            },
            "query": {
                "term": {
                    "type": "missing"
                }
            }
        }),
    )
    .await;
    assert_eq!(response.status, 400);
    assert_eq!(response.body.unwrap()["error"]["type"], "script_exception");

    let unchanged = call(&state, Method::GET, "/saved/_doc/1", Value::Null).await;
    assert_eq!(unchanged.status, 200);
    assert_eq!(
        unchanged.body.unwrap()["_source"]["namespaces"],
        json!(["default"])
    );
}
