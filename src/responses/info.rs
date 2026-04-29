use serde_json::json;

use crate::{config::Config, responses::Response};

pub fn root_info(config: &Config) -> Response {
    Response::json(
        200,
        json!({
            "name": "opensearch-lite",
            "cluster_name": "opensearch-lite",
            "cluster_uuid": "opensearch-lite-local",
            "version": {
                "distribution": "opensearch",
                "number": config.advertised_version,
                "build_type": "local",
                "build_hash": "opensearch-lite",
                "build_date": "2026-04-29T00:00:00Z",
                "build_snapshot": false,
                "lucene_version": "local",
                "minimum_wire_compatibility_version": "2.0.0",
                "minimum_index_compatibility_version": "2.0.0"
            },
            "tagline": "The OpenSearch Project: https://opensearch.org/"
        }),
    )
}
