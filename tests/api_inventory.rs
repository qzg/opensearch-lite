use http::Method;
use opensearch_lite::api_spec::{classify, inventory, Tier};

#[test]
fn route_inventory_has_required_core_entries() {
    let inventory = inventory();
    for required in [
        "info",
        "cluster.health",
        "indices.create",
        "indices.put_index_template",
        "indices.put_alias",
        "index",
        "bulk",
        "search",
    ] {
        assert!(
            inventory.iter().any(|route| route.name == required),
            "missing route inventory entry {required}"
        );
    }
    assert!(inventory.iter().any(|route| route.tier == Tier::BestEffort));
    assert!(
        inventory.len() >= 160,
        "inventory should be generated from the vendored OpenSearch spec"
    );
}

#[test]
fn vendored_opensearch_36_rest_spec_is_present() {
    let api_dir = std::path::Path::new("vendor/opensearch-rest-api-spec/rest-api-spec/api");
    let count = std::fs::read_dir(api_dir)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .ok()
                .and_then(|entry| entry.path().extension().map(|ext| ext == "json"))
                .unwrap_or(false)
        })
        .count();

    assert_eq!(count, 166);
    assert!(std::path::Path::new("vendor/opensearch-rest-api-spec/schema.json").exists());
}

#[test]
fn mutating_post_routes_fail_closed_and_read_routes_remain_fallback_eligible() {
    let delete_by_query = classify(&Method::POST, "/orders/_delete_by_query");
    assert_eq!(delete_by_query.api_name, "delete_by_query");
    assert_eq!(delete_by_query.tier, Tier::Unsupported);

    let delete_by_query_wrong_method = classify(&Method::GET, "/orders/_delete_by_query");
    assert_eq!(delete_by_query_wrong_method.api_name, "delete_by_query");
    assert_eq!(delete_by_query_wrong_method.tier, Tier::Unsupported);

    let reindex_wrong_method = classify(&Method::GET, "/_reindex");
    assert_eq!(reindex_wrong_method.api_name, "reindex");
    assert_eq!(reindex_wrong_method.tier, Tier::Unsupported);

    let count = classify(&Method::POST, "/orders/_count");
    assert_eq!(count.api_name, "count");
    assert_eq!(count.tier, Tier::Implemented);

    let unknown_post = classify(&Method::POST, "/_plugins/_unknown_write");
    assert_eq!(unknown_post.tier, Tier::Unsupported);
}

#[test]
fn document_route_shapes_match_opensearch_methods() {
    let auto_id = classify(&Method::POST, "/orders/_doc");
    assert_eq!(auto_id.api_name, "index");
    assert_eq!(auto_id.tier, Tier::Implemented);

    let put_auto_id = classify(&Method::PUT, "/orders/_doc");
    assert_eq!(put_auto_id.api_name, "index");
    assert_eq!(put_auto_id.tier, Tier::Unsupported);

    let extra_segments = classify(&Method::POST, "/orders/_doc/1/extra");
    assert_eq!(extra_segments.api_name, "index");
    assert_eq!(extra_segments.tier, Tier::Unsupported);
}
