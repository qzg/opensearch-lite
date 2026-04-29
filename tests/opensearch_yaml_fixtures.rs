use std::path::{Path, PathBuf};

#[test]
fn selected_upstream_yaml_fixtures_anchor_tranche_two_behavior() {
    for (fixture, needles) in FIXTURES.iter() {
        let path = Path::new(fixture);
        assert!(path.exists(), "missing vendored fixture {fixture}");
        let contents = std::fs::read_to_string(path).expect("fixture should be UTF-8");
        for needle in needles.iter() {
            assert!(
                contents.contains(needle),
                "{fixture} should contain fixture marker {needle:?}"
            );
        }
    }
}

#[test]
fn selected_upstream_yaml_fixtures_match_checked_out_opensearch_when_available() {
    let opensearch_dir = opensearch_source_dir();
    if !opensearch_dir.exists() {
        eprintln!(
            "skipping OpenSearch checkout fixture comparison; {} does not exist",
            opensearch_dir.display()
        );
        return;
    }

    for (fixture, _) in FIXTURES.iter() {
        let vendored_path = Path::new(fixture);
        let upstream_path = upstream_fixture_path(&opensearch_dir, fixture);

        assert!(
            upstream_path.exists(),
            "missing upstream OpenSearch fixture {} for vendored fixture {fixture}",
            upstream_path.display()
        );

        let vendored = std::fs::read_to_string(vendored_path)
            .unwrap_or_else(|error| panic!("failed to read {fixture}: {error}"));
        let upstream = std::fs::read_to_string(&upstream_path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", upstream_path.display()));

        assert_eq!(
            vendored,
            upstream,
            "{fixture} drifted from checked-out OpenSearch fixture {}",
            upstream_path.display()
        );
    }
}

const FIXTURES: &[(&str, &[&str])] = &[
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.refresh/10_basic.yml",
        &["indices.refresh:", "_shards.successful"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/bulk/50_refresh.yml",
        &["bulk:", "refresh: wait_for", "match: {count: 2}"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/mget/70_source_filtering.yml",
        &["mget:", "_source: false", "_source_includes"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/search.aggregation/20_terms.yml",
        &["terms", "aggregations.str_terms.buckets.0.doc_count"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/get_source/70_source_filtering.yml",
        &["get_source:", "_source_includes", "_source_excludes"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/get_source/85_source_missing.yml",
        &["_source: { enabled: false }", "catch:   missing"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.get_field_mapping/10_basic.yml",
        &["indices.get_field_mapping:", "include_defaults"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.get_field_mapping/50_field_wildcards.yml",
        &["fields: \"*\"", "fields: \"obj.i_*\""],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/indices.stats/10_index.yml",
        &["indices.stats:", "_shards.total", "fieldata"],
    ),
    (
        "vendor/opensearch-rest-api-spec/rest-api-spec/test/update/20_doc_upsert.yml",
        &["update:", "upsert:"],
    ),
];

fn opensearch_source_dir() -> PathBuf {
    std::env::var_os("OPENSEARCH_SOURCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../OpenSearch"))
}

fn upstream_fixture_path(opensearch_dir: &Path, vendored_fixture: &str) -> PathBuf {
    let fixture_tail = vendored_fixture
        .strip_prefix("vendor/opensearch-rest-api-spec/")
        .expect("fixture paths should be under the vendored OpenSearch REST spec");
    opensearch_dir
        .join("rest-api-spec/src/main/resources")
        .join(fixture_tail)
}
