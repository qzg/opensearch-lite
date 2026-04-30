use std::{env, fs, path::Path};

use opensearch_lite::agent::benchmark::{
    dry_run_report, live_model_discovery_report, load_fixtures, parse_candidate_sample,
    LiveBenchmarkConfig, OPENROUTER_CHAT_COMPLETIONS_URL,
};
use serde_json::json;

#[tokio::main]
async fn main() {
    let fixtures = load_fixtures(Path::new("fixtures/agent_fallback"))
        .expect("agent fallback benchmark fixtures load");
    let sample = json!({
        "models": [
            {
                "id": "dry-run-frontier",
                "provider": "sample",
                "quality_score": 95.0,
                "speed_score": 40.0,
                "cost_per_million_tokens": 20.0,
                "supports_tools": true,
                "supports_structured_outputs": true
            }
        ]
    });
    let candidates = parse_candidate_sample(&sample);
    let report = dry_run_report(&fixtures, &candidates);

    if env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH").ok().as_deref() != Some("1") {
        println!("{report}");
        println!(
            "agent_fallback_models: dry-run only; set OPENSEARCH_LITE_LIVE_AGENT_BENCH=1 with ignored local credentials for live OpenRouter/Artificial Analysis runs"
        );
        return;
    }

    let chat_completions_url = env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_ENDPOINT")
        .unwrap_or_else(|_| OPENROUTER_CHAT_COMPLETIONS_URL.to_string());
    let direct_endpoint =
        chat_completions_url.trim_end_matches('/') != OPENROUTER_CHAT_COMPLETIONS_URL;
    let model_ids = parse_model_ids();
    let openrouter_api_key = env::var("OPENROUTER_API_KEY").ok();
    let artificial_analysis_api_key = env::var("ARTIFICIAL_ANALYSIS_API_KEY").ok();
    let chat_api_key = env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            if direct_endpoint {
                None
            } else {
                openrouter_api_key.clone()
            }
        });
    if direct_endpoint {
        assert!(
            !model_ids.is_empty(),
            "direct endpoint benchmarks require OPENSEARCH_LITE_LIVE_AGENT_BENCH_MODELS"
        );
    } else {
        assert!(
            openrouter_api_key.is_some() && artificial_analysis_api_key.is_some(),
            "live benchmark requires OPENROUTER_API_KEY and ARTIFICIAL_ANALYSIS_API_KEY in the local environment"
        );
    }
    let report = live_model_discovery_report(
        &fixtures,
        LiveBenchmarkConfig {
            openrouter_api_key: openrouter_api_key.unwrap_or_default(),
            artificial_analysis_api_key: artificial_analysis_api_key.unwrap_or_default(),
            chat_completions_url: chat_completions_url.clone(),
            chat_api_key,
            max_candidates: env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(12),
            model_ids: model_ids.clone(),
            execute_fixture_prompts: env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTE")
                .ok()
                .as_deref()
                == Some("1"),
            execution_candidate_limit: env::var(
                "OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTION_MODEL_LIMIT",
            )
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3),
            fixture_limit: env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_FIXTURE_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(fixtures.len()),
            request_timeout_secs: env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(60),
            max_completion_tokens: env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_MAX_TOKENS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1600),
        },
    )
    .await
    .expect("live model discovery benchmark succeeds");
    fs::create_dir_all("reports/agent-fallback").expect("report directory can be created");
    let report_path = if direct_endpoint {
        "reports/agent-fallback/live-model-direct.json"
    } else if !model_ids.is_empty() {
        "reports/agent-fallback/live-model-shortlist.json"
    } else {
        "reports/agent-fallback/live-model-discovery.json"
    };
    fs::write(
        report_path,
        serde_json::to_vec_pretty(&report).expect("report serializes"),
    )
    .expect("report can be written");
    println!("{report}");
    println!("agent_fallback_models: wrote {report_path}");
}

fn parse_model_ids() -> Vec<String> {
    env::var("OPENSEARCH_LITE_LIVE_AGENT_BENCH_MODELS")
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}
