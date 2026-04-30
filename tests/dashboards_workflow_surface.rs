mod support;

use http::Method;
use serde_json::{json, Value};
use support::{call, ephemeral_state, ndjson_call};

const DATASET: &str = include_str!("fixtures/dashboards/discover_visualization_dataset.ndjson");

#[tokio::test]
async fn source_traceable_dashboards_fixture_workflow_passes_without_fallback() {
    let state = ephemeral_state();
    call(
        &state,
        Method::PUT,
        "/orders",
        json!({
            "mappings": {
                "properties": {
                    "status": { "type": "keyword" },
                    "created_at": { "type": "date" },
                    "total": { "type": "double" },
                    "quantity": { "type": "long" },
                    "customer": { "type": "keyword" },
                    "region": { "type": "keyword" },
                    "message": { "type": "text" }
                }
            }
        }),
    )
    .await;

    let bulk = ndjson_call(&state, Method::POST, "/_bulk", DATASET).await;
    assert_eq!(bulk.status, 200);
    assert_eq!(bulk.body.unwrap()["errors"], false);

    assert_eq!(
        call(&state, Method::HEAD, "/orders", Value::Null)
            .await
            .status,
        200
    );
    assert_eq!(
        call(&state, Method::GET, "/missing", Value::Null)
            .await
            .status,
        404
    );

    let caps = call(
        &state,
        Method::POST,
        "/orders/_field_caps?fields=*",
        json!({}),
    )
    .await;
    assert_eq!(caps.status, 200);
    assert_eq!(
        caps.body.unwrap()["fields"]["created_at"]["date"]["type"],
        "date"
    );

    assert_eq!(
        call(
            &state,
            Method::GET,
            "/_cat/plugins?format=json",
            Value::Null
        )
        .await
        .body
        .unwrap(),
        json!([])
    );
    assert_eq!(
        call(
            &state,
            Method::GET,
            "/_cat/templates?format=json",
            Value::Null
        )
        .await
        .body
        .unwrap(),
        json!([])
    );
    assert_eq!(
        call(&state, Method::GET, "/_cluster/stats", Value::Null)
            .await
            .body
            .unwrap()["indices"]["docs"]["count"],
        4
    );

    let discover = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "query": {
                "bool": {
                    "filter": [
                        { "terms": { "status": ["paid", "open"] } },
                        { "range": { "total": { "gte": 25 } } }
                    ],
                    "must_not": { "term": { "region": "west" } }
                }
            },
            "_source": ["status", "total", "region"],
            "sort": [{ "total": { "order": "desc" } }],
            "from": 0,
            "size": 2,
            "track_total_hits": true
        }),
    )
    .await;
    assert_eq!(discover.status, 200);
    let body = discover.body.unwrap();
    assert_eq!(body["hits"]["total"]["value"], 3);
    assert_eq!(body["hits"]["hits"][0]["_source"]["total"], 58.0);

    let visualization = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({
            "size": 0,
            "aggs": {
                "by_status": {
                    "terms": { "field": "status" },
                    "aggs": { "total": { "sum": { "field": "total" } } }
                },
                "by_day": { "date_histogram": { "field": "created_at", "calendar_interval": "day" } }
            }
        }),
    )
    .await;
    assert_eq!(visualization.status, 200);
    let body = visualization.body.unwrap();
    assert_eq!(
        body["aggregations"]["by_status"]["buckets"][0]["key"],
        "paid"
    );
    assert_eq!(
        body["aggregations"]["by_day"]["buckets"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let unsupported = call(
        &state,
        Method::POST,
        "/orders/_search",
        json!({ "query": { "percolate": {} } }),
    )
    .await;
    assert_eq!(unsupported.status, 400);
    assert_eq!(
        unsupported.body.unwrap()["error"]["type"],
        "x_content_parse_exception"
    );
}
