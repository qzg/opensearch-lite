use opensearch_lite::agent::benchmark::{
    dry_run_report, fixture_context, grade_agent_output, load_fixtures, parse_candidate_sample,
    rank_candidates, select_candidate_scores, LiveBenchmarkConfig, OPENROUTER_CHAT_COMPLETIONS_URL,
};
use serde_json::json;
use std::path::Path;

#[test]
fn dry_run_loads_agent_fallback_fixtures_without_network() {
    let fixtures = load_fixtures(Path::new("fixtures/agent_fallback")).unwrap();

    assert_eq!(fixtures.len(), 4);
    assert!(fixtures
        .iter()
        .any(|fixture| fixture.family == "tool_commit"));
    assert!(fixtures
        .iter()
        .any(|fixture| fixture.expected["requires_tool"] == "commit_mutations"));
}

#[test]
fn candidate_discovery_filters_for_tool_and_structured_output_support() {
    let candidates = parse_candidate_sample(&json!({
        "models": [
            {
                "id": "accurate-tools",
                "provider": "openrouter",
                "quality_score": 98.0,
                "speed_score": 20.0,
                "cost_per_million_tokens": 30.0,
                "supports_tools": true,
                "supports_structured_outputs": true
            },
            {
                "id": "no-tools",
                "provider": "openrouter",
                "quality_score": 99.0,
                "speed_score": 100.0,
                "cost_per_million_tokens": 1.0,
                "supports_tools": false,
                "supports_structured_outputs": true
            }
        ]
    }));

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, "accurate-tools");
}

#[test]
fn scoring_prioritizes_accuracy_over_speed_and_cost() {
    let candidates = parse_candidate_sample(&json!({
        "models": [
            {
                "id": "frontier-accurate",
                "provider": "openrouter",
                "quality_score": 96.0,
                "speed_score": 20.0,
                "cost_per_million_tokens": 30.0,
                "supports_tools": true,
                "supports_structured_outputs": true
            },
            {
                "id": "cheap-fast",
                "provider": "openrouter",
                "quality_score": 90.0,
                "speed_score": 200.0,
                "cost_per_million_tokens": 1.0,
                "supports_tools": true,
                "supports_structured_outputs": true
            }
        ]
    }));

    let scores = rank_candidates(&candidates);

    assert_eq!(scores[0].id, "frontier-accurate");
}

#[test]
fn named_candidate_selection_preserves_requested_order() {
    let candidates = parse_candidate_sample(&json!({
        "models": [
            {
                "id": "accurate-tools",
                "provider": "openrouter",
                "quality_score": 98.0,
                "speed_score": 20.0,
                "cost_per_million_tokens": 30.0,
                "supports_tools": true,
                "supports_structured_outputs": true
            },
            {
                "id": "cheap-fast",
                "provider": "openrouter",
                "quality_score": 90.0,
                "speed_score": 200.0,
                "cost_per_million_tokens": 1.0,
                "supports_tools": true,
                "supports_structured_outputs": true
            }
        ]
    }));

    let selected = select_candidate_scores(
        &candidates,
        &["cheap-fast".to_string(), "accurate-tools".to_string()],
        1,
    )
    .unwrap();

    assert_eq!(selected[0].id, "cheap-fast");
    assert_eq!(selected[1].id, "accurate-tools");
}

#[test]
fn dry_run_report_contains_no_secret_material() {
    let fixtures = load_fixtures(Path::new("fixtures/agent_fallback")).unwrap();
    let candidates = parse_candidate_sample(&json!({
        "models": [{
            "id": "frontier-accurate",
            "provider": "openrouter",
            "quality_score": 96.0,
            "speed_score": 20.0,
            "cost_per_million_tokens": 30.0,
            "supports_tools": true,
            "supports_structured_outputs": true
        }]
    }));

    let report = dry_run_report(&fixtures, &candidates).to_string();

    assert!(report.contains("frontier-accurate"));
    assert!(!report.contains("sk-or-"));
    assert!(!report.contains("aa_"));
}

#[test]
fn benchmark_config_detects_direct_chat_endpoints() {
    let mut config = LiveBenchmarkConfig {
        openrouter_api_key: String::new(),
        artificial_analysis_api_key: String::new(),
        chat_completions_url: OPENROUTER_CHAT_COMPLETIONS_URL.to_string(),
        chat_api_key: None,
        max_candidates: 1,
        model_ids: Vec::new(),
        execute_fixture_prompts: false,
        execution_candidate_limit: 1,
        fixture_limit: 1,
        request_timeout_secs: 1,
        max_completion_tokens: 1,
    };

    assert!(!config.uses_direct_chat_endpoint());

    config.chat_completions_url = "http://qzg-spark2:8000/v1/chat/completions".to_string();

    assert!(config.uses_direct_chat_endpoint());
}

#[test]
fn fixture_context_does_not_expose_live_grading_thresholds_to_candidate_model() {
    let fixture = load_fixtures(Path::new("fixtures/agent_fallback"))
        .unwrap()
        .into_iter()
        .find(|fixture| fixture.id == "benign_noop.cluster_reroute")
        .unwrap();

    let context = fixture_context(&fixture);

    assert!(fixture.expected.get("minimum_live_score").is_some());
    assert!(context
        .catalog
        .pointer("/expected_behavior/minimum_live_score")
        .is_none());
}

#[test]
fn fixture_grading_checks_wrapper_and_tool_shape() {
    let fixture = load_fixtures(Path::new("fixtures/agent_fallback"))
        .unwrap()
        .into_iter()
        .find(|fixture| fixture.id == "tool_commit.legacy_template")
        .unwrap();
    let raw = json!({
        "status": 200,
        "headers": {},
        "body": { "acknowledged": true },
        "confidence": 100,
        "failure_reason": null,
        "read_only": false,
        "tool_calls": [{
            "name": "commit_mutations",
            "arguments": {
                "mutations": [{
                    "kind": "put_registry_object",
                    "namespace": "legacy_template",
                    "name": "legacy-agent",
                    "raw": fixture.request["body"]
                }]
            }
        }]
    })
    .to_string();

    let grade = grade_agent_output(&fixture, &raw);

    assert!(grade.valid_wrapper);
    assert_eq!(grade.score, 1.0);
}
