# Agent Fallback Benchmarks

The benchmark harness is opt-in. Normal `cargo test` validates fixture schema,
candidate filtering, scoring order, and dry-run report generation without
network calls or paid model usage.

Run the dry path:

```sh
cargo test --test agent_benchmark_harness
cargo bench --bench agent_fallback_models
```

Live model discovery must use ignored local credentials, for example `.env`
loaded by your shell. The live path calls OpenRouter's models API and the
Artificial Analysis LLM stats API, merges tool/structured-output support with
quality, speed, and cost signals, and writes an ignored report. It does not
print API keys.

```sh
OPENSEARCH_LITE_LIVE_AGENT_BENCH=1 \
OPENROUTER_API_KEY=... \
ARTIFICIAL_ANALYSIS_API_KEY=... \
cargo bench --bench agent_fallback_models
```

To also run paid prompt fixtures against the top live candidates, opt in
explicitly:

```sh
OPENSEARCH_LITE_LIVE_AGENT_BENCH=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTE=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTION_MODEL_LIMIT=3 \
OPENROUTER_API_KEY=... \
ARTIFICIAL_ANALYSIS_API_KEY=... \
cargo bench --bench agent_fallback_models
```

To compare a cheaper named shortlist, pass comma-separated OpenRouter model
IDs. Named runs write `reports/agent-fallback/live-model-shortlist.json`.

```sh
OPENSEARCH_LITE_LIVE_AGENT_BENCH=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTE=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_MODELS=google/gemini-3-flash-preview,minimax/minimax-m2.7,deepseek/deepseek-v4-flash \
OPENROUTER_API_KEY=... \
ARTIFICIAL_ANALYSIS_API_KEY=... \
cargo bench --bench agent_fallback_models
```

To benchmark a local or self-hosted OpenAI-compatible endpoint, pass the chat
completions URL and explicit model IDs. This path does not call OpenRouter or
Artificial Analysis and writes `reports/agent-fallback/live-model-direct.json`.
Set `OPENSEARCH_LITE_LIVE_AGENT_BENCH_API_KEY` only when the endpoint requires
bearer auth.

```sh
OPENSEARCH_LITE_LIVE_AGENT_BENCH=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTE=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_ENDPOINT=http://qzg-spark2:8000/v1/chat/completions \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_MODELS=google/gemma-4-26B-A4B-it \
cargo bench --bench agent_fallback_models
```

Live fixture calls default to `max_tokens=1600` and a 60 second request
timeout. Tune those when comparing models that spend many tokens on hidden
reasoning or slow provider paths:

```sh
OPENSEARCH_LITE_LIVE_AGENT_BENCH=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_EXECUTE=1 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_TIMEOUT_SECS=20 \
OPENSEARCH_LITE_LIVE_AGENT_BENCH_MAX_TOKENS=2400 \
OPENROUTER_API_KEY=... \
ARTIFICIAL_ANALYSIS_API_KEY=... \
cargo bench --bench agent_fallback_models
```

The execution path asks each selected model for the fallback response wrapper
and grades status, read-only/write-tool policy, durable namespace, and raw body
preservation where the fixture requires them.

The same fixture set also backs an ignored live regression test for the
configured runtime model:

```sh
set -a
. ./.env
set +a
OPENSEARCH_LITE_LIVE_AGENT_TEST=1 \
cargo test --test live_agent_backend -- --ignored --test-threads=1
```

The live regression test fails per fixture when the model returns an invalid
wrapper or falls below that fixture's minimum score. Keep deterministic checks
in the fixture grader when possible; reserve Pro/frontier LLM-as-judge scoring
for semantic response-quality questions that cannot be checked structurally.

Reports should be written under `reports/agent-fallback/`, which is ignored by
Git. Promote conclusions into documentation only after reviewing the generated
report and redacting any provider response details that could expose secrets.
Each evaluation records request timeout/max-token settings, HTTP status,
provider error summaries, response/body byte counts, `finish_reason`, token
usage, and a `possibly_truncated` flag when the provider reports a length stop
or completion tokens reach the configured cap.

The ranking rule favors accuracy first, then speed, then cost. Fixture families
currently cover catalog/registry writes, benign local no-ops, the
`commit_mutations` tool boundary, and query-analysis scaffolding.
