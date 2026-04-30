use http::Method;
use opensearch_lite::api_spec::{classify, inventory, AccessClass, Tier};

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
    assert_eq!(delete_by_query.access, AccessClass::Write);

    let delete_by_query_wrong_method = classify(&Method::GET, "/orders/_delete_by_query");
    assert_eq!(delete_by_query_wrong_method.api_name, "delete_by_query");
    assert_eq!(delete_by_query_wrong_method.tier, Tier::Unsupported);

    let reindex_wrong_method = classify(&Method::GET, "/_reindex");
    assert_eq!(reindex_wrong_method.api_name, "reindex");
    assert_eq!(reindex_wrong_method.tier, Tier::Unsupported);

    let count = classify(&Method::POST, "/orders/_count");
    assert_eq!(count.api_name, "count");
    assert_eq!(count.tier, Tier::Implemented);
    assert_eq!(count.access, AccessClass::Read);

    let unknown_post = classify(&Method::POST, "/_plugins/_unknown_write");
    assert_eq!(unknown_post.tier, Tier::Unsupported);
    assert_eq!(unknown_post.access, AccessClass::Write);

    let unknown_security_post = classify(&Method::POST, "/_plugins/_security/unknown");
    assert_eq!(unknown_security_post.tier, Tier::Unsupported);
    assert_eq!(unknown_security_post.access, AccessClass::Admin);
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

#[test]
fn refresh_and_existence_routes_use_specific_api_names() {
    let refresh = classify(&Method::POST, "/orders/_refresh");
    assert_eq!(refresh.api_name, "indices.refresh");
    assert_eq!(refresh.tier, Tier::Implemented);
    assert_eq!(refresh.access, AccessClass::Write);

    let template_exists = classify(&Method::HEAD, "/_index_template/orders");
    assert_eq!(template_exists.api_name, "indices.exists_index_template");
    assert_eq!(template_exists.tier, Tier::Implemented);
    assert_eq!(template_exists.access, AccessClass::Read);

    let alias_exists = classify(&Method::HEAD, "/orders/_alias/orders-read");
    assert_eq!(alias_exists.api_name, "indices.exists_alias");
    assert_eq!(alias_exists.tier, Tier::Implemented);
    assert_eq!(alias_exists.access, AccessClass::Read);
}

#[test]
fn generated_inventory_contains_access_classes() {
    let inventory = inventory();
    let read_routes = inventory
        .iter()
        .filter(|route| route.access == AccessClass::Read)
        .count();
    let write_routes = inventory
        .iter()
        .filter(|route| route.access == AccessClass::Write)
        .count();
    let admin_routes = inventory
        .iter()
        .filter(|route| route.access == AccessClass::Admin)
        .count();

    assert!(read_routes > 0, "inventory should include read routes");
    assert!(write_routes > 0, "inventory should include write routes");
    assert!(admin_routes > 0, "inventory should include admin routes");

    let search = inventory
        .iter()
        .find(|route| route.name == "search")
        .expect("missing search route");
    assert_eq!(search.access, AccessClass::Read);

    let create_index = inventory
        .iter()
        .find(|route| route.name == "indices.create")
        .expect("missing indices.create route");
    assert_eq!(create_index.access, AccessClass::Write);
}

#[test]
fn tranche_three_routes_are_implemented_by_specific_names() {
    let source = classify(&Method::GET, "/orders/_source/1");
    assert_eq!(source.api_name, "get_source");
    assert_eq!(source.tier, Tier::Implemented);

    let source_exists = classify(&Method::HEAD, "/orders/_source/1");
    assert_eq!(source_exists.api_name, "exists_source");
    assert_eq!(source_exists.tier, Tier::Implemented);

    let field_mapping = classify(&Method::GET, "/orders/_mapping/field/status");
    assert_eq!(field_mapping.api_name, "indices.get_field_mapping");
    assert_eq!(field_mapping.tier, Tier::Implemented);

    let stats = classify(&Method::GET, "/orders/_stats");
    assert_eq!(stats.api_name, "indices.stats");
    assert_eq!(stats.tier, Tier::Implemented);

    let cat_indices = classify(&Method::GET, "/_cat/indices/orders");
    assert_eq!(cat_indices.api_name, "cat.indices");
    assert_eq!(cat_indices.tier, Tier::BestEffort);
}

#[test]
fn tranche_three_generated_inventory_marks_manual_handlers() {
    let inventory = inventory();
    for implemented in [
        "get_source",
        "exists_source",
        "indices.get_field_mapping",
        "indices.stats",
    ] {
        let route = inventory
            .iter()
            .find(|route| route.name == implemented)
            .unwrap_or_else(|| panic!("missing route inventory entry {implemented}"));
        assert_eq!(
            route.tier,
            Tier::Implemented,
            "route {implemented} should be discoverable as implemented"
        );
    }
}

#[test]
fn tranche_three_routes_reject_invalid_extra_segments() {
    let bad_stats = classify(&Method::GET, "/orders/_stats/docs/extra");
    assert_eq!(bad_stats.api_name, "indices.stats");
    assert_eq!(bad_stats.tier, Tier::Unsupported);

    let bad_field_mapping = classify(&Method::GET, "/orders/_mapping/field/status/extra");
    assert_eq!(bad_field_mapping.api_name, "indices.get_field_mapping");
    assert_eq!(bad_field_mapping.tier, Tier::Unsupported);
}
