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
    assert!(inventory.iter().any(|route| route.tier == Tier::Mocked));
    assert!(inventory.iter().any(|route| route.tier == Tier::AgentWrite));
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
    assert_eq!(delete_by_query.tier, Tier::Implemented);
    assert_eq!(delete_by_query.access, AccessClass::Write);

    let delete_by_query_wrong_method = classify(&Method::GET, "/orders/_delete_by_query");
    assert_eq!(delete_by_query_wrong_method.api_name, "delete_by_query");
    assert_eq!(delete_by_query_wrong_method.tier, Tier::Unsupported);

    for path in ["/_delete_by_query", "/orders/_delete_by_query/extra"] {
        let route = classify(&Method::POST, path);
        assert_eq!(route.tier, Tier::Unsupported, "{path}");
        assert_eq!(route.access, AccessClass::Write, "{path}");
    }

    let update_by_query = classify(&Method::POST, "/orders/_update_by_query");
    assert_eq!(update_by_query.api_name, "update_by_query");
    assert_eq!(update_by_query.tier, Tier::Implemented);
    assert_eq!(update_by_query.access, AccessClass::Write);

    for path in ["/_update_by_query", "/orders/_update_by_query/extra"] {
        let route = classify(&Method::POST, path);
        assert_eq!(route.tier, Tier::Unsupported, "{path}");
        assert_eq!(route.access, AccessClass::Write, "{path}");
    }

    let reindex = classify(&Method::POST, "/_reindex");
    assert_eq!(reindex.api_name, "reindex");
    assert_eq!(reindex.tier, Tier::Implemented);
    assert_eq!(reindex.access, AccessClass::Write);

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
fn known_malformed_get_shapes_fail_closed_instead_of_agent_fallback() {
    for (path, api_name) in [
        ("/orders/_delete_by_query/extra", "delete_by_query"),
        ("/orders/_update_by_query/extra", "update_by_query"),
        ("/orders/_validate/query/extra", "indices.validate_query"),
        ("/_ingest/pipeline/pipeline-a/extra", "ingest.get_pipeline"),
        ("/_search/pipeline/pipeline-a/extra", "search_pipeline.get"),
        ("/_scripts/script-a/context/extra", "get_script"),
    ] {
        let route = classify(&Method::GET, path);
        assert_eq!(route.api_name, api_name, "{path}");
        assert_eq!(route.tier, Tier::Unsupported, "{path}");
        assert_ne!(route.tier, Tier::AgentRead, "{path}");
    }

    let dangling = classify(&Method::GET, "/_dangling/abc");
    assert_eq!(dangling.tier, Tier::Unsupported);
    assert_eq!(dangling.access, AccessClass::Admin);
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

    let field_caps = inventory
        .iter()
        .find(|route| route.name == "field_caps")
        .expect("missing field_caps route");
    assert_eq!(field_caps.access, AccessClass::Read);

    let create_index = inventory
        .iter()
        .find(|route| route.name == "indices.create")
        .expect("missing indices.create route");
    assert_eq!(create_index.access, AccessClass::Write);

    let task_get = inventory
        .iter()
        .find(|route| route.name == "tasks.get")
        .expect("missing tasks.get route");
    assert_eq!(task_get.access, AccessClass::Read);
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

#[test]
fn core_write_routes_reject_invalid_extra_segments() {
    for (method, path, api_name, access) in [
        (
            Method::POST,
            "/orders/_bulk/extra",
            "bulk",
            AccessClass::Write,
        ),
        (
            Method::GET,
            "/orders/_bulk/extra",
            "bulk",
            AccessClass::Read,
        ),
        (
            Method::PUT,
            "/orders/_mapping/extra",
            "indices.get_mapping",
            AccessClass::Write,
        ),
        (
            Method::PUT,
            "/orders/_settings/extra",
            "indices.get_settings",
            AccessClass::Write,
        ),
        (
            Method::PUT,
            "/_index_template/template-extra/extra",
            "indices.put_index_template",
            AccessClass::Write,
        ),
        (
            Method::PUT,
            "/orders/_alias/orders-read/extra",
            "indices.put_alias",
            AccessClass::Write,
        ),
        (
            Method::POST,
            "/_aliases/extra",
            "indices.put_alias",
            AccessClass::Write,
        ),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, api_name, "{method} {path}");
        assert_eq!(route.tier, Tier::Unsupported, "{method} {path}");
        assert_eq!(route.access, access, "{method} {path}");
    }
}

#[test]
fn dashboards_metadata_routes_have_specific_read_and_write_classes() {
    let exists = classify(&Method::HEAD, "/orders");
    assert_eq!(exists.api_name, "indices.exists");
    assert_eq!(exists.tier, Tier::Implemented);
    assert_eq!(exists.access, AccessClass::Read);

    for (method, path) in [
        (Method::GET, "/_field_caps"),
        (Method::POST, "/_field_caps"),
        (Method::GET, "/orders/_field_caps"),
        (Method::POST, "/orders/_field_caps"),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, "field_caps", "{method} {path}");
        assert_eq!(route.tier, Tier::Implemented, "{method} {path}");
        assert_eq!(route.access, AccessClass::Read, "{method} {path}");
    }

    for path in [
        "/_cat/plugins",
        "/_cat/templates",
        "/_cluster/stats",
        "/_resolve/index/orders*",
    ] {
        let route = classify(&Method::GET, path);
        assert_eq!(route.tier, Tier::Implemented, "{path}");
        assert_eq!(route.access, AccessClass::Read, "{path}");
    }

    for path in ["/_nodes", "/_nodes/http", "/_nodes/local-node/http"] {
        let route = classify(&Method::GET, path);
        assert_eq!(route.api_name, "nodes.info", "{path}");
        assert_eq!(route.tier, Tier::BestEffort, "{path}");
        assert_eq!(route.access, AccessClass::Read, "{path}");
    }

    for path in [
        "/_nodes/stats",
        "/_nodes/local-node/stats",
        "/_nodes/stats/indices",
        "/_nodes/stats/indices/docs",
        "/_nodes/local-node/stats/indices/docs",
    ] {
        let route = classify(&Method::GET, path);
        assert_eq!(route.api_name, "nodes.stats", "{path}");
        assert_eq!(route.tier, Tier::BestEffort, "{path}");
        assert_eq!(route.access, AccessClass::Read, "{path}");
    }

    for (path, api_name) in [
        ("/_nodes/local-node/stats/indices/docs/extra", "nodes.stats"),
        ("/_nodes/stats/indices/docs/extra", "nodes.stats"),
        ("/_nodes/local-node/http/extra", "nodes.info"),
    ] {
        let route = classify(&Method::GET, path);
        assert_eq!(route.api_name, api_name, "{path}");
        assert_eq!(route.tier, Tier::Unsupported, "{path}");
        assert_eq!(route.access, AccessClass::Read, "{path}");
    }

    let delete_template = classify(&Method::DELETE, "/_template/legacy");
    assert_eq!(delete_template.api_name, "indices.delete_template");
    assert_eq!(delete_template.tier, Tier::Implemented);
    assert_eq!(delete_template.access, AccessClass::Write);

    let alias_singular = classify(&Method::POST, "/_alias");
    assert_eq!(alias_singular.api_name, "indices.update_aliases");
    assert_eq!(alias_singular.tier, Tier::Implemented);
    assert_eq!(alias_singular.access, AccessClass::Write);

    for (method, path, api_name) in [
        (Method::POST, "/_search/scroll", "scroll"),
        (
            Method::GET,
            "/_search/scroll/opensearch-lite-scroll:1",
            "scroll",
        ),
        (
            Method::POST,
            "/_search/scroll/opensearch-lite-scroll:1",
            "scroll",
        ),
        (Method::DELETE, "/_search/scroll", "clear_scroll"),
        (
            Method::DELETE,
            "/_search/scroll/opensearch-lite-scroll:1",
            "clear_scroll",
        ),
        (Method::GET, "/_tasks/opensearch-lite-task:1", "tasks.get"),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, api_name, "{method} {path}");
        assert_eq!(route.tier, Tier::Implemented, "{method} {path}");
        assert_eq!(route.access, AccessClass::Read, "{method} {path}");
    }
}

#[test]
fn dashboards_metadata_wrong_methods_fail_closed() {
    for (method, path, api_name) in [
        (Method::PUT, "/_field_caps", "field_caps"),
        (Method::POST, "/_cat/plugins", "cat.plugins"),
        (Method::DELETE, "/_cluster/stats", "cluster.stats"),
        (
            Method::POST,
            "/_resolve/index/orders*",
            "indices.resolve_index",
        ),
        (
            Method::POST,
            "/_plugins/_query/_datasources",
            "query.datasources",
        ),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, api_name, "{method} {path}");
        assert_eq!(route.tier, Tier::Unsupported, "{method} {path}");
        assert_ne!(route.tier, Tier::AgentRead, "{method} {path}");
    }

    let legacy_get = classify(&Method::GET, "/_template/legacy");
    assert_eq!(legacy_get.api_name, "indices.get_template");
    assert_eq!(legacy_get.tier, Tier::Implemented);
    assert_eq!(legacy_get.access, AccessClass::Read);
}

#[test]
fn mocked_and_write_fallback_routes_are_explicit_tiers() {
    let cluster_settings = classify(&Method::PUT, "/_cluster/settings");
    assert_eq!(cluster_settings.api_name, "cluster.put_settings");
    assert_eq!(cluster_settings.tier, Tier::Mocked);
    assert_eq!(cluster_settings.access, AccessClass::Admin);

    let security_account = classify(&Method::GET, "/_plugins/_security/api/account");
    assert_eq!(security_account.api_name, "security.account");
    assert_eq!(security_account.tier, Tier::Mocked);
    assert_eq!(security_account.access, AccessClass::Read);

    let security_account_write = classify(&Method::PUT, "/_plugins/_security/api/account");
    assert_eq!(security_account_write.api_name, "security.account");
    assert_eq!(security_account_write.tier, Tier::Unsupported);
    assert_eq!(security_account_write.access, AccessClass::Admin);

    let query_datasources = classify(&Method::GET, "/_plugins/_query/_datasources");
    assert_eq!(query_datasources.api_name, "query.datasources");
    assert_eq!(query_datasources.tier, Tier::Mocked);
    assert_eq!(query_datasources.access, AccessClass::Read);

    let malformed_query_datasources = classify(&Method::GET, "/_plugins/_query/_datasources/extra");
    assert_eq!(malformed_query_datasources.api_name, "query.datasources");
    assert_eq!(malformed_query_datasources.tier, Tier::Unsupported);
    assert_eq!(malformed_query_datasources.access, AccessClass::Read);
    assert_ne!(malformed_query_datasources.tier, Tier::AgentRead);

    for (method, path, api_name) in [
        (Method::POST, "/_cluster/reroute", "cluster.reroute"),
        (Method::POST, "/orders/_flush", "indices.flush"),
        (Method::POST, "/orders/_forcemerge", "indices.forcemerge"),
        (Method::POST, "/orders/_cache/clear", "indices.clear_cache"),
        (
            Method::POST,
            "/_delete_by_query/opensearch-lite-task:1/_rethrottle",
            "delete_by_query_rethrottle",
        ),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, api_name, "{method} {path}");
        assert_eq!(route.tier, Tier::Mocked, "{method} {path}");
    }

    for (method, path, api_name) in [
        (Method::POST, "/orders/_close", "indices.close"),
        (Method::PUT, "/orders/_block/write", "indices.add_block"),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, api_name, "{method} {path}");
        assert_eq!(route.tier, Tier::Unsupported, "{method} {path}");
    }

    for (method, path, api_name) in [
        (
            Method::PUT,
            "/_component_template/component-a",
            "cluster.put_component_template",
        ),
        (
            Method::PUT,
            "/_ingest/pipeline/pipeline-a",
            "ingest.put_pipeline",
        ),
        (Method::PUT, "/_scripts/script-a", "put_script"),
        (
            Method::PUT,
            "/_search/pipeline/pipeline-a",
            "search_pipeline.put",
        ),
        (Method::PUT, "/_scripts/script-a/search", "put_script"),
    ] {
        let route = classify(&method, path);
        assert_eq!(route.api_name, api_name, "{method} {path}");
        assert_eq!(route.tier, Tier::Implemented, "{method} {path}");
    }

    let legacy_template = classify(&Method::PUT, "/_template/legacy-a");
    assert_eq!(legacy_template.api_name, "indices.put_template");
    assert_eq!(legacy_template.tier, Tier::AgentWrite);

    let painless_execute = classify(&Method::POST, "/_scripts/painless/_execute");
    assert_eq!(painless_execute.api_name, "scripts_painless_execute");
    assert_eq!(painless_execute.tier, Tier::Unsupported);
}
