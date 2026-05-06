use std::{
    cmp::Ordering,
    fs, io,
    path::{Path, PathBuf},
    time::Instant,
};

use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    agent::{
        prompt,
        tools::{tool_catalog, AgentToolCall},
        validation::AgentResponseWrapper,
        AgentRequestContext,
    },
    api_spec::{self, Tier},
};

pub const OPENROUTER_MODELS_URL: &str =
    "https://openrouter.ai/api/v1/models?supported_parameters=tools";
pub const OPENROUTER_CHAT_COMPLETIONS_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkFixture {
    pub id: String,
    pub family: String,
    pub prompt_variant: String,
    pub request: Value,
    pub expected: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateModel {
    pub id: String,
    pub provider: String,
    pub quality_score: f64,
    pub speed_score: f64,
    pub cost_per_million_tokens: f64,
    pub supports_tools: bool,
    pub supports_structured_outputs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateScore {
    pub id: String,
    pub accuracy: f64,
    pub speed_score: f64,
    pub cost_score: f64,
    pub total: f64,
}

#[derive(Debug, Clone)]
pub struct LiveBenchmarkConfig {
    pub openrouter_api_key: String,
    pub artificial_analysis_api_key: String,
    pub chat_completions_url: String,
    pub chat_api_key: Option<String>,
    pub max_candidates: usize,
    pub model_ids: Vec<String>,
    pub execute_fixture_prompts: bool,
    pub execution_candidate_limit: usize,
    pub fixture_limit: usize,
    pub request_timeout_secs: u64,
    pub max_completion_tokens: u32,
}

impl LiveBenchmarkConfig {
    pub fn uses_direct_chat_endpoint(&self) -> bool {
        normalize_url(&self.chat_completions_url) != normalize_url(OPENROUTER_CHAT_COMPLETIONS_URL)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FixtureGrade {
    pub fixture_id: String,
    pub valid_wrapper: bool,
    pub score: f64,
    pub checks: Vec<FixtureCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FixtureCheck {
    pub name: String,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct OpenRouterModel {
    id: String,
    name: String,
    provider: String,
    cost_per_million_tokens: f64,
}

#[derive(Debug, Clone)]
struct AnalysisModel {
    name: String,
    slug: String,
    creator: String,
    quality_score: f64,
    speed_score: f64,
    cost_per_million_tokens: f64,
}

pub fn load_fixtures(dir: &Path) -> io::Result<Vec<BenchmarkFixture>> {
    let mut paths = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<PathBuf>>>()?;
    paths.sort();
    let mut fixtures = Vec::new();
    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if value.is_array() {
            fixtures.extend(
                serde_json::from_value::<Vec<BenchmarkFixture>>(value)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        } else {
            fixtures.push(
                serde_json::from_value::<BenchmarkFixture>(value)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        }
    }
    Ok(fixtures)
}

pub fn parse_candidate_sample(value: &Value) -> Vec<CandidateModel> {
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some(CandidateModel {
                id: model.get("id")?.as_str()?.to_string(),
                provider: model
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                quality_score: number(model, "quality_score"),
                speed_score: number(model, "speed_score"),
                cost_per_million_tokens: number(model, "cost_per_million_tokens"),
                supports_tools: model
                    .get("supports_tools")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                supports_structured_outputs: model
                    .get("supports_structured_outputs")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .filter(|model| model.supports_tools && model.supports_structured_outputs)
        .collect()
}

pub fn rank_candidates(candidates: &[CandidateModel]) -> Vec<CandidateScore> {
    let max_speed = candidates
        .iter()
        .map(|candidate| candidate.speed_score)
        .fold(0.0, f64::max)
        .max(1.0);
    let max_cost = candidates
        .iter()
        .map(|candidate| candidate.cost_per_million_tokens)
        .fold(0.0, f64::max)
        .max(1.0);
    let mut scores = candidates
        .iter()
        .map(|candidate| {
            let accuracy = candidate.quality_score / 100.0;
            let speed_score = candidate.speed_score / max_speed;
            let cost_score = 1.0 - (candidate.cost_per_million_tokens / max_cost);
            let total = accuracy.mul_add(10_000.0, speed_score.mul_add(100.0, cost_score));
            CandidateScore {
                id: candidate.id.clone(),
                accuracy,
                speed_score,
                cost_score,
                total,
            }
        })
        .collect::<Vec<_>>();
    scores.sort_by(|left, right| {
        right
            .total
            .partial_cmp(&left.total)
            .unwrap_or(Ordering::Equal)
    });
    scores
}

pub fn select_candidate_scores(
    candidates: &[CandidateModel],
    requested_model_ids: &[String],
    max_candidates: usize,
) -> Result<Vec<CandidateScore>, String> {
    let ranked = rank_candidates(candidates);
    if requested_model_ids.is_empty() {
        return Ok(ranked.into_iter().take(max_candidates).collect());
    }

    let by_id = ranked
        .into_iter()
        .map(|score| (score.id.clone(), score))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut selected = Vec::with_capacity(requested_model_ids.len());
    let mut missing = Vec::new();
    for id in requested_model_ids {
        match by_id.get(id) {
            Some(score) => selected.push(score.clone()),
            None => missing.push(id.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "requested benchmark models were not found in matched OpenRouter/Artificial Analysis candidates: {}",
            missing.join(", ")
        ));
    }
    Ok(selected)
}

pub fn dry_run_report(fixtures: &[BenchmarkFixture], candidates: &[CandidateModel]) -> Value {
    json!({
        "mode": "dry_run",
        "fixture_count": fixtures.len(),
        "families": fixtures.iter().map(|fixture| fixture.family.clone()).collect::<std::collections::BTreeSet<_>>(),
        "candidate_scores": rank_candidates(candidates)
    })
}

pub async fn live_model_discovery_report(
    fixtures: &[BenchmarkFixture],
    config: LiveBenchmarkConfig,
) -> Result<Value, String> {
    let direct_endpoint = config.uses_direct_chat_endpoint();
    let (openrouter_model_count, analysis_model_count, matched_candidate_count, scores, sources) =
        if direct_endpoint {
            let scores = direct_candidate_scores(&config.model_ids)?;
            (
                0,
                0,
                scores.len(),
                scores,
                json!({
                    "chat_completions": config.chat_completions_url,
                    "model_selection": "explicit MAINSTACK_SEARCH_LIVE_AGENT_BENCH_MODELS for a direct OpenAI-compatible endpoint"
                }),
            )
        } else {
            let openrouter = fetch_json(
                OPENROUTER_MODELS_URL,
                Some((
                    "authorization",
                    format!("Bearer {}", config.openrouter_api_key),
                )),
            )
            .await?;
            let analysis = fetch_json(
                "https://artificialanalysis.ai/api/v2/data/llms/models",
                Some(("x-api-key", config.artificial_analysis_api_key.clone())),
            )
            .await?;
            let openrouter_models = parse_openrouter_models(&openrouter);
            let analysis_models = parse_analysis_models(&analysis);
            let candidates = merge_live_candidates(&openrouter_models, &analysis_models);
            let scores =
                select_candidate_scores(&candidates, &config.model_ids, config.max_candidates)?;
            (
                openrouter_models.len(),
                analysis_models.len(),
                candidates.len(),
                scores,
                json!({
                    "openrouter": OPENROUTER_MODELS_URL,
                    "artificial_analysis": "https://artificialanalysis.ai/api/v2/data/llms/models",
                    "artificial_analysis_attribution": "Data from Artificial Analysis: https://artificialanalysis.ai/"
                }),
            )
        };
    let fixture_evaluations = if config.execute_fixture_prompts {
        run_live_fixture_prompts(fixtures, &scores, &config).await?
    } else {
        Vec::new()
    };

    Ok(json!({
        "mode": match (direct_endpoint, !config.model_ids.is_empty(), config.execute_fixture_prompts) {
            (true, _, true) => "live_model_benchmark_direct",
            (true, _, false) => "live_model_discovery_direct",
            (false, true, true) => "live_model_benchmark_named",
            (false, true, false) => "live_model_discovery_named",
            (false, false, true) => "live_model_benchmark",
            (false, false, false) => "live_model_discovery",
        },
        "fixture_count": fixtures.len(),
        "families": fixtures.iter().map(|fixture| fixture.family.clone()).collect::<std::collections::BTreeSet<_>>(),
        "requested_model_ids": config.model_ids,
        "sources": sources,
        "openrouter_tool_structured_model_count": openrouter_model_count,
        "artificial_analysis_model_count": analysis_model_count,
        "matched_candidate_count": matched_candidate_count,
        "candidate_scores": scores,
        "fixture_execution": {
            "enabled": config.execute_fixture_prompts,
            "candidate_limit": config.execution_candidate_limit,
            "fixture_limit": config.fixture_limit,
            "timeout_secs": config.request_timeout_secs,
            "max_completion_tokens": config.max_completion_tokens,
            "evaluations": fixture_evaluations
        }
    }))
}

fn direct_candidate_scores(requested_model_ids: &[String]) -> Result<Vec<CandidateScore>, String> {
    if requested_model_ids.is_empty() {
        return Err(
            "direct OpenAI-compatible benchmark endpoints require MAINSTACK_SEARCH_LIVE_AGENT_BENCH_MODELS"
                .to_string(),
        );
    }
    Ok(requested_model_ids
        .iter()
        .map(|id| CandidateScore {
            id: id.clone(),
            accuracy: 0.0,
            speed_score: 0.0,
            cost_score: 0.0,
            total: 0.0,
        })
        .collect())
}

pub fn grade_agent_output(fixture: &BenchmarkFixture, raw: &str) -> FixtureGrade {
    let wrapper = match serde_json::from_str::<AgentResponseWrapper>(raw) {
        Ok(wrapper) => wrapper,
        Err(error) => {
            return FixtureGrade {
                fixture_id: fixture.id.clone(),
                valid_wrapper: false,
                score: 0.0,
                checks: vec![FixtureCheck {
                    name: "wrapper_json".to_string(),
                    passed: false,
                    reason: format!("response was not a valid wrapper: {error}"),
                }],
            };
        }
    };
    let mut checks = Vec::new();
    if let Some(expected_status) = fixture.expected.get("status").and_then(Value::as_u64) {
        checks.push(FixtureCheck {
            name: "status".to_string(),
            passed: wrapper.status as u64 == expected_status,
            reason: format!("expected status {expected_status}, got {}", wrapper.status),
        });
    }
    if fixture
        .expected
        .get("must_not_mutate")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let committed = wrapper.tool_calls.iter().any(is_commit_call);
        checks.push(FixtureCheck {
            name: "no_mutation".to_string(),
            passed: !committed && wrapper.read_only,
            reason: "read-only fixture must not include commit_mutations".to_string(),
        });
    }
    if let Some(tool_name) = fixture
        .expected
        .get("requires_tool")
        .and_then(Value::as_str)
    {
        let has_tool = wrapper.tool_calls.iter().any(|call| call.name == tool_name);
        checks.push(FixtureCheck {
            name: "required_tool".to_string(),
            passed: has_tool,
            reason: format!("expected tool call {tool_name}"),
        });
    }
    if let Some(namespace) = fixture
        .expected
        .get("durable_namespace")
        .and_then(Value::as_str)
    {
        let namespace_match = committed_mutations(&wrapper.tool_calls)
            .iter()
            .any(|mutation| mutation.get("namespace").and_then(Value::as_str) == Some(namespace));
        checks.push(FixtureCheck {
            name: "durable_namespace".to_string(),
            passed: namespace_match,
            reason: format!("expected a mutation in namespace {namespace}"),
        });
    }
    if fixture
        .expected
        .get("must_preserve_schema")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let expected_body = fixture.request.get("body").unwrap_or(&Value::Null);
        let raw_match = committed_mutations(&wrapper.tool_calls)
            .iter()
            .any(|mutation| mutation.get("raw") == Some(expected_body));
        checks.push(FixtureCheck {
            name: "raw_body_preserved".to_string(),
            passed: raw_match,
            reason: "durable mutation raw payload should match the request body".to_string(),
        });
    }
    if let Some(expected_valid) = fixture.expected.get("valid").and_then(Value::as_bool) {
        checks.push(FixtureCheck {
            name: "valid_flag".to_string(),
            passed: wrapper.body.get("valid").and_then(Value::as_bool) == Some(expected_valid),
            reason: format!("expected body.valid={expected_valid}"),
        });
    }
    if let Some(tier) = fixture.expected.get("tier").and_then(Value::as_str) {
        checks.push(FixtureCheck {
            name: "tier".to_string(),
            passed: wrapper
                .body
                .get("mainstack_search")
                .and_then(|note| note.get("tier"))
                .and_then(Value::as_str)
                == Some(tier),
            reason: format!("expected body.mainstack_search.tier={tier}"),
        });
    }

    let passed = checks.iter().filter(|check| check.passed).count();
    let score = if checks.is_empty() {
        1.0
    } else {
        passed as f64 / checks.len() as f64
    };
    FixtureGrade {
        fixture_id: fixture.id.clone(),
        valid_wrapper: true,
        score,
        checks,
    }
}

async fn run_live_fixture_prompts(
    fixtures: &[BenchmarkFixture],
    scores: &[CandidateScore],
    config: &LiveBenchmarkConfig,
) -> Result<Vec<Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;
    let mut evaluations = Vec::new();
    for score in scores.iter().take(config.execution_candidate_limit.max(1)) {
        for fixture in fixtures.iter().take(config.fixture_limit.max(1)) {
            eprintln!(
                "agent_fallback_models: running model={} fixture={}",
                score.id, fixture.id
            );
            evaluations.push(run_live_fixture_prompt(&client, config, &score.id, fixture).await);
        }
    }
    Ok(evaluations)
}

async fn run_live_fixture_prompt(
    client: &reqwest::Client,
    config: &LiveBenchmarkConfig,
    model_id: &str,
    fixture: &BenchmarkFixture,
) -> Value {
    let context = fixture_context(fixture);
    let messages = prompt::messages(&context);
    let payload = json!({
        "model": model_id,
        "messages": messages,
        "temperature": 0,
        "max_tokens": config.max_completion_tokens,
        "response_format": { "type": "json_object" }
    });
    let started = Instant::now();
    let mut request = client.post(&config.chat_completions_url);
    if let Some(api_key) = config
        .chat_api_key
        .as_deref()
        .filter(|api_key| !api_key.trim().is_empty())
    {
        request = request.bearer_auth(api_key);
    }
    let response = request.json(&payload).send().await;
    let latency_ms = started.elapsed().as_millis();
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return json!({
                "model": model_id,
                "fixture": fixture.id,
                "latency_ms": latency_ms,
                "request": benchmark_request_metadata(config),
                "error": {
                    "stage": "request",
                    "kind": request_error_kind(&error),
                    "message": error.to_string(),
                    "is_timeout": error.is_timeout(),
                    "is_connect": error.is_connect(),
                    "is_body": error.is_body(),
                    "is_decode": error.is_decode()
                }
            });
        }
    };
    let status = response.status().as_u16();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return json!({
                "model": model_id,
                "fixture": fixture.id,
                "latency_ms": latency_ms,
                "http_status": status,
                "request": benchmark_request_metadata(config),
                "error": {
                    "stage": "read_body",
                    "kind": request_error_kind(&error),
                    "message": error.to_string(),
                    "is_timeout": error.is_timeout(),
                    "is_body": error.is_body()
                }
            });
        }
    };
    let response_body_bytes = bytes.len();
    let value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return json!({
                "model": model_id,
                "fixture": fixture.id,
                "latency_ms": latency_ms,
                "http_status": status,
                "response_body_bytes": response_body_bytes,
                "request": benchmark_request_metadata(config),
                "error": {
                    "stage": "decode_json",
                    "message": error.to_string()
                }
            });
        }
    };
    if !(200..300).contains(&status) {
        return json!({
            "model": model_id,
            "fixture": fixture.id,
            "latency_ms": latency_ms,
            "http_status": status,
            "response_body_bytes": response_body_bytes,
            "request": benchmark_request_metadata(config),
            "provider_error": provider_error_summary(&value),
            "raw_response_shape": response_shape(&value)
        });
    }
    let first_choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let finish_reason = first_choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    let content = value
        .pointer("/choices/0/message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let usage = value.get("usage").cloned().unwrap_or(Value::Null);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let possibly_truncated = finish_reason
        .as_deref()
        .is_some_and(|reason| matches!(reason, "length" | "max_tokens" | "content_filter"))
        || output_tokens >= u64::from(config.max_completion_tokens);
    json!({
        "model": model_id,
        "fixture": fixture.id,
        "latency_ms": latency_ms,
        "http_status": status,
        "response_body_bytes": response_body_bytes,
        "content_bytes": content.len(),
        "finish_reason": finish_reason,
        "possibly_truncated": possibly_truncated,
        "request": benchmark_request_metadata(config),
        "usage": usage,
        "grade": grade_agent_output(fixture, content),
    })
}

fn benchmark_request_metadata(config: &LiveBenchmarkConfig) -> Value {
    json!({
        "chat_completions_url": config.chat_completions_url,
        "timeout_secs": config.request_timeout_secs,
        "max_completion_tokens": config.max_completion_tokens,
        "response_format": "json_object"
    })
}

fn request_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else if error.is_status() {
        "status"
    } else if error.is_builder() {
        "builder"
    } else if error.is_request() {
        "request"
    } else {
        "unknown"
    }
}

fn provider_error_summary(value: &Value) -> Value {
    value
        .get("error")
        .cloned()
        .unwrap_or_else(|| json!({ "message": "non-success HTTP response without error field" }))
}

fn response_shape(value: &Value) -> Value {
    json!({
        "has_choices": value.get("choices").is_some(),
        "choice_count": value
            .get("choices")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        "has_usage": value.get("usage").is_some(),
        "top_level_keys": value.as_object()
            .map(|object| object.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    })
}

pub fn fixture_context(fixture: &BenchmarkFixture) -> AgentRequestContext {
    let method = fixture
        .request
        .get("method")
        .and_then(Value::as_str)
        .and_then(|method| method.parse::<Method>().ok())
        .unwrap_or(Method::GET);
    let path = fixture
        .request
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("/")
        .to_string();
    let route = api_spec::classify(&method, &path);
    let write_enabled = route.tier == Tier::AgentWrite
        || fixture.expected.get("requires_tool").is_some()
        || fixture.expected.get("durable_namespace").is_some();
    AgentRequestContext {
        method: method.as_str().to_string(),
        path,
        query: Value::Object(Default::default()),
        body: fixture.request.get("body").cloned().unwrap_or(Value::Null),
        api_name: route.api_name.to_string(),
        route_tier: if write_enabled {
            "agent_write_fallback_eligible".to_string()
        } else {
            "agent_fallback_eligible".to_string()
        },
        catalog: json!({
            "benchmark_fixture": fixture.id,
            "expected_behavior": fixture_expected_behavior(fixture)
        }),
        tools: tool_catalog(route.api_name, write_enabled),
    }
}

fn fixture_expected_behavior(fixture: &BenchmarkFixture) -> Value {
    let Some(object) = fixture.expected.as_object() else {
        return fixture.expected.clone();
    };
    let mut expected = object.clone();
    expected.remove("minimum_live_score");
    Value::Object(expected)
}

fn is_commit_call(call: &AgentToolCall) -> bool {
    call.name == "commit_mutations"
}

fn committed_mutations(calls: &[AgentToolCall]) -> Vec<&Value> {
    calls
        .iter()
        .filter(|call| is_commit_call(call))
        .flat_map(|call| {
            call.arguments
                .get("mutations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect()
}

async fn fetch_json(url: &str, header: Option<(&str, String)>) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;
    let mut request = client.get(url);
    if let Some((name, value)) = header {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("failed to fetch {url}: {error}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("failed to read {url}: {error}"))?;
    if !status.is_success() {
        return Err(format!("{url} returned HTTP {status}"));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("{url} returned invalid JSON: {error}"))
}

fn parse_openrouter_models(value: &Value) -> Vec<OpenRouterModel> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let supported = model
                .get("supported_parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<_>>();
            if !supported.contains("tools")
                || !(supported.contains("response_format")
                    || supported.contains("structured_outputs"))
            {
                return None;
            }
            let id = model.get("id")?.as_str()?.to_string();
            let provider = id.split('/').next().unwrap_or("unknown").to_string();
            let pricing = model.get("pricing").unwrap_or(&Value::Null);
            let prompt = token_price_per_million(pricing.get("prompt"));
            let completion = token_price_per_million(pricing.get("completion"));
            Some(OpenRouterModel {
                id,
                name: model
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                provider,
                cost_per_million_tokens: blended_cost(prompt, completion),
            })
        })
        .collect()
}

fn parse_analysis_models(value: &Value) -> Vec<AnalysisModel> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let evaluations = model.get("evaluations").unwrap_or(&Value::Null);
            let quality_score = evaluations
                .get("artificial_analysis_coding_index")
                .and_then(Value::as_f64)
                .or_else(|| {
                    evaluations
                        .get("artificial_analysis_intelligence_index")
                        .and_then(Value::as_f64)
                })
                .unwrap_or_default();
            let pricing = model.get("pricing").unwrap_or(&Value::Null);
            Some(AnalysisModel {
                name: model.get("name")?.as_str()?.to_string(),
                slug: model
                    .get("slug")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                creator: model
                    .get("model_creator")
                    .and_then(|creator| creator.get("slug").or_else(|| creator.get("name")))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                quality_score,
                speed_score: model
                    .get("median_output_tokens_per_second")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                cost_per_million_tokens: pricing
                    .get("price_1m_blended_3_to_1")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn merge_live_candidates(
    openrouter_models: &[OpenRouterModel],
    analysis_models: &[AnalysisModel],
) -> Vec<CandidateModel> {
    openrouter_models
        .iter()
        .filter_map(|openrouter| {
            let analysis = analysis_models
                .iter()
                .max_by_key(|analysis| model_match_score(openrouter, analysis))?;
            if model_match_score(openrouter, analysis) == 0 {
                return None;
            }
            Some(CandidateModel {
                id: openrouter.id.clone(),
                provider: openrouter.provider.clone(),
                quality_score: analysis.quality_score,
                speed_score: analysis.speed_score,
                cost_per_million_tokens: if analysis.cost_per_million_tokens > 0.0 {
                    analysis.cost_per_million_tokens
                } else {
                    openrouter.cost_per_million_tokens
                },
                supports_tools: true,
                supports_structured_outputs: true,
            })
        })
        .collect()
}

fn model_match_score(openrouter: &OpenRouterModel, analysis: &AnalysisModel) -> i32 {
    let openrouter_name = normalize(&openrouter.name);
    let openrouter_id = normalize(&openrouter.id);
    let openrouter_provider = normalize(&openrouter.provider);
    let analysis_name = normalize(&analysis.name);
    let analysis_slug = normalize(&analysis.slug);
    let analysis_creator = normalize(&analysis.creator);

    let mut score = 0;
    if !analysis_creator.is_empty()
        && (openrouter_provider.contains(&analysis_creator)
            || analysis_creator.contains(&openrouter_provider))
    {
        score += 2;
    }
    if !analysis_name.is_empty()
        && (openrouter_name.contains(&analysis_name)
            || analysis_name.contains(&openrouter_name)
            || openrouter_id.contains(&analysis_name))
    {
        score += 3;
    }
    if !analysis_slug.is_empty() && openrouter_id.contains(&analysis_slug) {
        score += 3;
    }
    score
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_url(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn token_price_per_million(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| {
            value
                .as_str()
                .and_then(|value| value.parse::<f64>().ok())
                .or_else(|| value.as_f64())
        })
        .unwrap_or_default()
        * 1_000_000.0
}

fn blended_cost(prompt_per_million: f64, completion_per_million: f64) -> f64 {
    prompt_per_million.mul_add(0.75, completion_per_million * 0.25)
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or_default()
}
