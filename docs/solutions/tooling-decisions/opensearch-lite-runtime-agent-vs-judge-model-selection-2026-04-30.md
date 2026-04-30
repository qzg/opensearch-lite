---
title: OpenSearch Lite Runtime Agent Versus Judge Model Selection
date: 2026-04-30
category: tooling-decisions
module: opensearch-lite agent fallback benchmarking
problem_type: tooling_decision
component: tooling
severity: medium
applies_when:
  - "Selecting models for runtime agent fallback and benchmark evaluation"
  - "Adding LLM-as-judge grading for OpenSearch-compatible response quality"
  - "Comparing cheap fast hosted models against stronger Pro or frontier models"
related_components:
  - assistant
  - testing_framework
  - documentation
tags:
  - opensearch-lite
  - agent-fallback
  - model-selection
  - llm-as-judge
  - openrouter
  - benchmarking
---

# OpenSearch Lite Runtime Agent Versus Judge Model Selection

## Context

OpenSearch Lite uses a configured OpenAI-compatible model endpoint to answer
fallback requests in local development. The project also needs model-assisted
benchmarking for cases where deterministic fixture checks are not enough to
judge whether an OpenSearch-shaped response is semantically correct.

Those are different jobs. Treating one model as both the runtime fallback and
the evaluator would make benchmark results less trustworthy and could bias the
system toward cheap models that are good enough to answer requests but not
strong enough to grade subtle compatibility behavior.

## Guidance

Keep two explicit model roles:

- **Runtime fallback model:** used by OpenSearch Lite while serving fallback
  API requests. Optimize for correctness first, then latency, then cost. Based
  on the current benchmark run, `deepseek/deepseek-v4-flash` through OpenRouter
  is the current best cheap/fast runtime candidate.
- **Judge model:** used only by benchmark/evaluation tooling when deterministic
  checks are insufficient. Use a Pro or frontier-grade model here. Do not use a
  Flash-grade runtime model as the judge for semantic compatibility decisions.

The deterministic fixture grader should stay model-free where possible. It can
assert wrapper validity, status codes, read/write policy, tool-call presence,
durable namespace, target names, and raw-body preservation without another LLM.
Only introduce LLM-as-judge for qualitative comparisons such as whether an
unsupported API response gives an agent caller a faithful OpenSearch-compatible
shape and useful remediation guidance.

## Why This Matters

Runtime fallback has to be fast and inexpensive because it may sit directly on
the request path. Judge evaluation has to be more conservative because it
decides whether a candidate model or prompt is safe to trust. If the same
cheap model both answers requests and grades its own response class, benchmark
scores can overstate quality and hide compatibility gaps.

Keeping the roles separate also makes reports easier to interpret. A benchmark
can say "DeepSeek Flash is the candidate runtime model" while a Pro/frontier
judge scores semantic correctness independently.

## When to Apply

- When changing `.env` or server flags for runtime fallback.
- When adding or expanding live model benchmark fixtures.
- When adding LLM-as-judge scoring beyond deterministic fixture assertions.
- When comparing a local OpenAI-compatible endpoint against OpenRouter-hosted
  models.

## Examples

Runtime fallback configuration:

```sh
OPENSEARCH_LITE_AGENT_ENDPOINT=https://openrouter.ai/api/v1/chat/completions
OPENSEARCH_LITE_AGENT_MODEL=deepseek/deepseek-v4-flash
OPENSEARCH_LITE_AGENT_TOKEN_ENV=OPENROUTER_API_KEY
```

Live runtime regression tests should exercise the configured fallback backend
and fail on deterministic contract checks:

```sh
set -a
. ./.env
set +a
OPENSEARCH_LITE_LIVE_AGENT_TEST=1 \
cargo test --test live_agent_backend -- --ignored --test-threads=1
```

LLM-as-judge scoring should be added as a separate benchmark/evaluation option
with its own model configuration, for example:

```sh
OPENSEARCH_LITE_JUDGE_MODEL=<pro-or-frontier-model>
OPENSEARCH_LITE_RUNTIME_MODEL=deepseek/deepseek-v4-flash
```

## Related

- [OpenSearch Lite Agent Write Fallback And Durable Replay Hardening](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/solutions/security-issues/opensearch-lite-agent-write-fallback-durable-replay-hardening-2026-04-30.md)
- [docs/agent-fallback-benchmarks.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/agent-fallback-benchmarks.md)
- [docs/agent-fallback.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/agent-fallback.md)
