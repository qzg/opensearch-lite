---
title: OpenSearch Lite P1 Code Review Hardening
date: 2026-04-29
category: security-issues
module: opensearch-lite
problem_type: security_issue
component: assistant
symptoms:
  - "Mutating POST routes could invoke agent fallback instead of failing closed"
  - "Agent fallback could expose too much local catalog data to a configured agent endpoint"
  - "Unsupported bulk methods and malformed bulk source lines could still mutate state"
  - "_create and bulk create could overwrite existing documents instead of returning conflicts"
  - "Large accepted documents could exceed the advertised memory budget"
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - tooling
  - testing_framework
tags:
  - opensearch-lite
  - agent-fallback
  - bulk-api
  - write-safety
  - memory-limit
  - durability
---

# OpenSearch Lite P1 Code Review Hardening

## Problem

OpenSearch Lite's local compatibility layer needed P1 hardening before it could be trusted as a development substitute for OpenSearch. The risky area was not a single bug; it was a cluster of boundary failures around route classification, agent fallback eligibility, bulk parsing, create semantics, memory admission, and blocking storage work.

## Symptoms

- Known mutating APIs such as `_delete_by_query` or `_reindex` could be classified broadly enough to reach runtime agent fallback instead of returning an unsupported error.
- Agent fallback context could include more local data than the request needed, which is dangerous because the endpoint may be a configured local or cloud OpenAI-compatible host.
- Path-based bulk routing meant wrong methods such as `GET /_bulk` could reach the mutating bulk handler.
- `_create` and bulk `create` did not preserve OpenSearch conflict semantics for existing IDs.
- Malformed or missing bulk source lines could be treated like empty documents, turning parse failures into writes.
- `--memory-limit` existed as configuration, but large stored documents needed to be rejected before state was committed.
- Durable storage work needed to avoid blocking Tokio async workers while mutation logs and snapshots were written.

## What Didn't Work

- Relying on agent response validation alone was insufficient. The request must fail closed before fallback when the route shape is known to be mutating or method-mismatched.
- Treating partial OpenSearch support as "ask the fallback agent" was unsafe. Known-but-unsupported write and control APIs need structured 501 responses, not synthesized success.
- Defaulting malformed bulk item bodies to `{}` made the system look permissive, but it corrupted the mutation boundary by converting invalid input into valid writes.
- Session-history review showed that the first pass was too broad at the route boundary: known-but-unsupported mutating/control routes needed explicit unsupported behavior before any fallback path was considered.

## Solution

Route classification now fails closed. `src/api_spec/mod.rs` recognizes implemented read/write routes by method, returns `Unsupported` for known wrong-method or known-but-unimplemented mutating/control routes, and reserves `AgentRead` for read-oriented inventory routes or unknown `GET` requests.

```rust
if *method == Method::GET {
    return RouteMatch {
        api_name: "agent.read",
        tier: Tier::AgentRead,
    };
}
```

Agent fallback context is scoped in `src/api/mod.rs`. Unknown fallback requests receive metadata only, while known read fallback can include bounded documents only for validated target indices. This keeps a configured agent endpoint useful without sending the full local catalog by default.

Bulk and document writes are planned before mutation. `handle_bulk` parses item actions into `BulkPlan` values, converts malformed source lines into item-level errors, and sends only valid `WriteOperation`s to storage. Unsupported bulk methods never reach `handle_bulk` because classification rejects them first.

Create semantics moved into storage validation. `_create` and bulk `create` use `WriteOperation::CreateDocument`, and `Store::validate_mutation` returns `version_conflict_engine_exception` when the target ID already exists.

```rust
if matches!(mutation, Mutation::CreateDocument { .. }) && existing {
    return Err(StoreError::new(
        409,
        "version_conflict_engine_exception",
        format!("document [{id}] already exists"),
    ));
}
```

Storage admission validates candidate state before commit. `validate_memory` estimates stored state bytes and rejects writes that would exceed `memory_limit_bytes`. Bulk writes go through `apply_write_operations`, so valid planned operations are committed together, malformed source lines and storage validation failures remain item-level failures, and malformed action JSON or action metadata returns a request-level parse error before storage planning.

Mutating API handlers call storage through `run_store`, which uses `tokio::task::spawn_blocking`. Durable JSONL append and snapshot work can still serialize local writes, but it no longer runs directly on Tokio async workers.

## Why This Works

The root cause was missing validation at multiple trust boundaries. OpenSearch Lite has to mimic OpenSearch's HTTP surface, but the local implementation cannot let compatibility gaps blur method safety, write semantics, data exposure, or resource limits.

The fix makes each boundary explicit:

- Route classification decides whether a request is implemented, best-effort, fallback-eligible, or unsupported before any handler runs.
- Agent fallback is read-only and receives metadata-only context unless a known read route identifies target indices.
- Bulk parsing separates invalid source/action lines from valid store operations.
- Storage validates create conflicts, document limits, and estimated memory against candidate state before publishing.
- Blocking durable work is moved off the async runtime worker path.

## Prevention

- Keep regression tests around the named review findings: mutating POST routes do not enter fallback, wrong-method bulk does not mutate, `_create` conflicts preserve the original document, malformed bulk items do not create documents, and memory-limit rejection leaves no partial index behind.
- Preserve method-aware route inventory checks when adding OpenSearch APIs. A path match is not enough; method mismatches for known routes should usually be unsupported rather than fallback-eligible.
- Treat agent fallback context as a data-exposure boundary. Unknown routes should stay metadata-only, and known read routes should include only bounded, targeted documents.
- Add compatibility features with client and parity coverage at the same time, especially for API semantics that official clients depend on.
- Re-run focused code review after safety fixes; the useful follow-up signal is whether route inventory, fallback context, memory accounting, and durable write behavior are covered by tests and observable behavior (session history).

## Related Issues

- [docs/agent-fallback.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/agent-fallback.md:1)
- [docs/supported-apis.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/supported-apis.md:22)
- [docs/compatibility.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/compatibility.md:8)
- [docs/plans/2026-04-29-001-feat-opensearch-lite-implementation-plan.md](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/plans/2026-04-29-001-feat-opensearch-lite-implementation-plan.md:573)
- [OpenSearch Lite Snapshot Reserved Selector Delete Hardening](opensearch-lite-snapshot-reserved-selector-delete-hardening-2026-05-04.md) covers the later snapshot-specific selector/control-token fail-closed boundary.
