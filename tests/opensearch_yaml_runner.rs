mod support;

#[path = "support/yaml_rest_runner.rs"]
mod yaml_rest_runner;

use yaml_rest_runner::run_selected_yaml_tests;

#[tokio::test]
async fn selected_upstream_yaml_rest_tests_run_against_local_router() {
    let cases = [
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.refresh/10_basic.yml",
            &[
                "Indices refresh test _all",
                "Indices refresh test empty array",
                "Indices refresh test no-match wildcard",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/bulk/50_refresh.yml",
            &[
                "refresh=true immediately makes changes are visible in search",
                "refresh=empty string immediately makes changes are visible in search",
                "refresh=wait_for waits until changes are visible in search",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/mget/70_source_filtering.yml",
            &[
                "Source filtering -  true/false",
                "Source filtering -  include field",
                "Source filtering -  include nested field",
                "Source filtering -  exclude field",
                "Source filtering -  ids and true/false",
                "Source filtering -  ids and include field",
                "Source filtering -  ids and include nested field",
                "Source filtering -  ids and exclude field",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/search.aggregation/20_terms.yml",
            &["Basic test"][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/get_source/70_source_filtering.yml",
            &["Source filtering"][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/get_source/85_source_missing.yml",
            &[
                "Missing document source with catch",
                "Missing document source with ignore",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.get_field_mapping/10_basic.yml",
            &[
                "Get field mapping with no index",
                "Get field mapping by index only",
                "Get field mapping by field, with another field that doesn't exist",
                "Get field mapping with include_defaults",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.get_field_mapping/50_field_wildcards.yml",
            &[
                "Get field mapping with * for fields",
                "Get field mapping with t* for fields",
                "Get field mapping with *t1 for fields",
                "Get field mapping with wildcarded relative names",
                "Get field mapping should work using '_all' for index",
                "Get field mapping should work using '*' for index",
                "Get field mapping should work using comma_separated values for indices",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.stats/10_index.yml",
            &[
                "Index - blank",
                "Index - all",
                "Index - star",
                "Index - star, no match",
                "Index - one index",
                "Index - multi-index",
                "Index - pattern",
                "Indices stats unrecognized parameter",
            ][..],
        ),
        (
            "vendor/opensearch-rest-api-spec/rest-api-spec/test/update/20_doc_upsert.yml",
            &["Doc upsert"][..],
        ),
    ];

    for (fixture, selected_tests) in cases {
        run_selected_yaml_tests(fixture, selected_tests).await;
    }
}
