---
title: OpenSearch Lite Agent Write Fallback And Durable Replay Hardening
date: 2026-04-30
category: security-issues
module: opensearch-lite agent fallback and durable storage
problem_type: security_issue
component: assistant
symptoms:
  - "Write-capable agent fallback could commit mutations before wrapper confidence, status, and write-policy validation"
  - "Agent tool calls could target data outside the request path or persist a different raw payload than the caller sent"
  - "Known malformed route shapes and stateful control APIs could look successful without deterministic implementation"
  - "Durable replay and snapshot compaction could exceed memory limits or fail after a crash in the metadata/log transition window"
  - "Live model benchmarks initially failed the response wrapper because the prompt did not pin confidence to an integer contract"
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - tooling
  - database
  - testing_framework
  - documentation
tags:
  - opensearch-lite
  - agent-fallback
  - write-safety
  - mutation-log
  - snapshot-replay
  - model-benchmarking
  - route-classification
  - durability
---

# OpenSearch Lite Agent Write Fallback And Durable Replay Hardening

## Problem

OpenSearch Lite added raw agent-owned responses and write-enabled fallback for trusted local development, but the first implementation left several trust boundaries too soft. A fallback model could be useful for unsupported OpenSearch API families only if the server, not the model, remained responsible for durable mutation scope, route safety, memory admission, and recovery semantics.

## Symptoms

- `handle_agent_write` could execute `commit_mutations` before validating response confidence, status, and read/write intent.
- `indices.put_template` fallback validation originally allowed the model to commit a different legacy template name or raw body than the request path/body.
- Prefix-style route checks let extra path segments such as `/index/_bulk/extra` or malformed GET shapes reach fallback or handlers instead of failing closed.
- Snapshot replay validated the final in-memory database after load, but a large active log could allocate past `--memory-limit` before diagnostics ran.
- Snapshot compaction could trim the mutation log after writing metadata with `log_compacted=false`; a crash before the second metadata write then made restart look corrupt because the high-water transaction was no longer in the active log.
- The live benchmark harness initially reached OpenRouter successfully, but all top models returned `confidence` as floats or words because the prompt did not explicitly require an integer `0..100`.

## What Didn't Work

- Validating the agent wrapper after tool execution was backwards. Once a server-validated tool commits durable state, rejecting the wrapper only rejects the response, not the side effect.
- Naming a tool `commit_mutations` was not enough of a write boundary. The server had to prove that every model-supplied mutation matched the current API name, request path target, and raw request body.
- Treating all positive mock responses as harmless was too broad. `indices.close` and `indices.add_block` are stateful protection APIs; acknowledging them without storing and enforcing closed/block state gives clients false safety.
- Post-load memory diagnostics gave good error messages only after the damage was already done. Durable replay needed bounded validation while mutations were applied.
- Mutation-log compaction that only removed old records could not distinguish "valid compacted log" from "snapshot metadata points at a transaction that is missing from an uncompacted log."
- The model benchmark's first pass showed that JSON response formatting instructions were insufficient. Models inferred confidence as a probability unless the prompt specified the exact numeric type.

## Solution

Write fallback now validates the wrapper before any durable tool call. `handle_agent_write` parses the wrapper, calls `validate_write_wrapper_before_tools`, then executes tool calls in a blocking task only after the wrapper passes confidence, status, and read/write checks.

```rust
let wrapper = match state.agent.complete_raw(context).await {
    Ok(wrapper) => wrapper,
    Err(error) => return failure_response(error),
};
if let Err(error) =
    validate_write_wrapper_before_tools(&wrapper, state.config.agent.confidence_threshold)
{
    return failure_response(error);
}
```

The tool layer accumulates all requested mutations, requires exactly one `commit_mutations` call for writes, validates the whole mutation list against an `AgentWriteScope`, and commits once. For `indices.put_template`, the only accepted mutation is one `PutRegistryObject` in the `legacy_template` namespace whose name equals the `{name}` path segment and whose `raw` payload exactly equals the original request body.

```rust
if matches!(
    &mutations[0],
    Mutation::PutRegistryObject { namespace, name, raw }
        if namespace == "legacy_template" && name == expected_name && raw == expected_raw
) {
    return Ok(());
}
```

Route classification was tightened around exact shapes. Write-shaped routes with extra segments fail closed, malformed GETs for known route families no longer drop into generic read fallback, script context routes are handled consistently, and the write fallback allowlist no longer accepts `*`. Stateful protection APIs such as `indices.close` and `indices.add_block` were removed from the mocked tier until the server can store and enforce their state.

Durable replay now accepts a memory-budget validator. `load_durable_state` validates snapshot state immediately after snapshot load and calls `mutation_log::replay_validating` or `replay_after_validating` so memory diagnostics run after applied replay mutations instead of only after the entire database is allocated.

```rust
mutation_log::replay_after_validating(
    mutation_log_path,
    db,
    metadata.last_transaction_id.as_deref(),
    |db| validate_loaded_database_memory(db, memory_limit_bytes),
)?;
```

Mutation-log compaction now writes a high-water marker at the start of the compacted log. If metadata still says `log_compacted=false` after a crash, `replay_after` can treat the marker for the snapshot transaction as proof that the log was validly compacted rather than corrupt. Missing high-water records without a marker still return `InvalidData`, preserving corruption detection.

```json
{"version":1,"transaction":"compacted_after","id":"<snapshot-high-water-transaction>"}
```

Mutation-log fsync moved to a deliberate one-second batch window. Since this development server accepts best-effort delayed fsync, shutdown performs one final best-effort sync when the dirty flag is still set so clean shutdown does not skip the pending batch.

The model benchmark prompt now pins the wrapper schema, including `confidence` as an integer from `0` to `100`, and adds tests for the prompt contract. After the prompt fix, the live benchmark identified `google/gemini-3.1-pro-preview-customtools` as the best current candidate: tied for first by discovery score, valid wrappers on `4/4` fixtures, complete passes on `3/4`, average latency around `2682ms`, and a reported fixture-run cost of about `$0.05173`.

## Why This Works

The root cause was missing validation across server/model and durable-state boundaries. The fallback model can draft an OpenSearch-compatible response and propose tool calls, but it cannot be the authority for whether a mutation is safe, in-scope, atomic, memory-admissible, or recoverable after restart.

The fix splits responsibilities clearly:

- The model owns only the wrapper content and proposed tool calls.
- The server validates wrapper confidence and write intent before side effects.
- The server validates each proposed mutation against request-bound scope.
- The store commits all accepted fallback mutations atomically through existing storage validation.
- Durable boot validates memory while replaying, not only after replay completes.
- Snapshot/log compaction leaves enough metadata in the log to make crash recovery deterministic.
- Benchmark prompts encode the exact wrapper contract so model selection measures useful behavior rather than prompt ambiguity.

## Prevention

- Keep agent fallback tests close to the handler boundary. Tests should call `AppState::with_agent` with static model responses and assert both response behavior and absence/presence of committed registry data.
- Any new write-enabled fallback API needs a request-bound `AgentWriteScope`, one atomic commit path, and tests proving wrong names, wrong bodies, multiple commit calls, and invalid wrappers do not mutate state.
- Treat route classification as a security boundary. Known malformed shapes should become structured unsupported responses, not generic fallback.
- Do not mock stateful protection APIs unless state is persisted and enforced. Positive no-op mocks are appropriate for immaterial cluster operations, not for APIs that promise blocking or closed-index protection.
- Durable replay changes need crash-window tests. Include cases for snapshot metadata/log mismatches, compacted-log high-water markers, torn final records, and memory-budget failures during replay.
- Run the live benchmark after prompt changes. If all models fail the same schema check, fix the prompt or wrapper parser before drawing model-quality conclusions.

## Related Issues

- Moderate overlap: [OpenSearch Lite P1 Code Review Hardening](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/solutions/security-issues/opensearch-lite-p1-code-review-hardening-2026-04-29.md:1) covers the earlier read-only fallback, bulk, create-conflict, and memory-admission hardening; this doc covers write-enabled fallback, tool scope, durable replay, compaction recovery, and model benchmark prompt lessons.
- Moderate overlap: [OpenSearch Lite Dashboards Migration API Surface Hardening](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/solutions/integration-issues/opensearch-lite-dashboards-migration-api-surface-hardening-2026-04-30.md:1) covers by-query, scroll, reindex, and Dashboards migration route safety; this doc covers fallback write tools and durable storage recovery.
- Related guidance: [docs/agent-fallback.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/agent-fallback.md:1)
- Related benchmark report: [reports/agent-fallback/live-model-discovery.json](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/reports/agent-fallback/live-model-discovery.json:1)
- Related plan: [docs/plans/2026-04-30-003-feat-agent-fallback-write-support-plan.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/plans/2026-04-30-003-feat-agent-fallback-write-support-plan.md:1)
