use std::path::Path;

#[test]
fn selected_upstream_yaml_fixtures_anchor_tranche_two_behavior() {
    let fixtures = [
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.refresh/10_basic.yml",
            &["indices.refresh:", "_shards.successful"][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/bulk/50_refresh.yml",
            &["bulk:", "refresh: wait_for", "match: {count: 2}"][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/mget/70_source_filtering.yml",
            &["mget:", "_source: false", "_source_includes"][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/search.aggregation/20_terms.yml",
            &["terms", "aggregations.str_terms.buckets.0.doc_count"][..],
        ),
    ];

    for (fixture, needles) in fixtures {
        let path = Path::new(fixture);
        assert!(path.exists(), "missing vendored fixture {fixture}");
        let contents = std::fs::read_to_string(path).expect("fixture should be UTF-8");
        for needle in needles {
            assert!(
                contents.contains(needle),
                "{fixture} should contain fixture marker {needle:?}"
            );
        }
    }
}
