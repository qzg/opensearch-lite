---
title: OpenSearch Lite Dashboards Migration API Surface Hardening
date: 2026-04-30
category: integration-issues
module: opensearch-lite dashboards migration api
problem_type: integration_issue
component: tooling
symptoms:
  - "Scroll contexts could retain every cloned hit from large migration searches"
  - "Known by-query mutation route shapes were accepted too broadly or rejected incorrectly"
  - "Delete-by-query without a query could default to match_all and delete too much"
  - "Saved-object workspace migrations missed array-backed term queries and accepted near-miss scripts"
  - "Reindex create semantics and by-query mutation planning could diverge from OpenSearch"
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - database
  - testing_framework
  - documentation
tags:
  - opensearch-lite
  - opensearch-dashboards
  - api-compatibility
  - saved-object-migrations
  - scroll
  - by-query
  - reindex
  - route-classification
---

# OpenSearch Lite Dashboards Migration API Surface Hardening

## Problem

OpenSearch Lite's OpenSearch Dashboards saved-object migration surface had several code-review failures across scroll, by-query mutations, reindex, route classification, and term query evaluation. The defects made the local development server too permissive in some mutating paths, too incomplete in some client-compatible read paths, and too willing to retain or mutate more state than the caller explicitly requested.

## Symptoms

- Scroll searches could retain all cloned hits in runtime cursor state instead of retaining only bounded remaining pages.
- Global by-query paths such as `/_delete_by_query`, global update paths, and extra path forms such as `/index/_delete_by_query/extra` did not fail closed as exact unsupported mutation shapes.
- `delete_by_query` without a `query` could implicitly behave like `match_all`, making an empty body destructive.
- `update_by_query` accepted saved-object scripts by substring and validated during per-document application, so near-miss scripts could mutate documents or succeed when no documents matched.
- Workspace saved-object migrations using `term` against array fields did not match documents such as `{ "workspaces": ["default", "workspace-a"] }`.
- `_reindex` ignored `dest.op_type=create`, so existing destination documents could be overwritten instead of producing version conflicts.

## What Didn't Work

- Broad path matching was too permissive for mutation APIs. Route classification is a safety boundary, so exact supported shapes need to be listed and global or extra mutation shapes need structured unsupported responses.
- Defaulting missing by-query requests to `match_all` was unsafe for `delete_by_query`. Even if a compatible `update_by_query` migration can default to all documents, a delete operation needs explicit caller intent.
- Substring script allowlisting was too weak. A script that merely mentions `namespaces` or `params['namespace']` is not necessarily the supported saved-object namespace removal migration.
- Planning matches from a read snapshot and mutating later left a stale read/mutate gap, and it also pushed by-query and reindex through a heavier cloned-operation path.
- Treating scroll as a full-result cache made large migration searches retain more data than the local memory posture should allow.

## Solution

Route classification now recognizes only the intended by-query and scroll shapes. Global by-query routes are unsupported, exact index-scoped `POST` routes are implemented, and path-form scroll requests are classified as deterministic read routes.

```rust
if segments.as_slice() == ["_delete_by_query"] {
    return unsupported_method("delete_by_query", method);
}
if matches!(segments.as_slice(), [_, "_delete_by_query"]) {
    return match *method {
        Method::POST => route("delete_by_query", Tier::Implemented, AccessClass::Write),
        _ => unsupported_method("delete_by_query", method),
    };
}
```

Scroll capture is bounded before runtime retention. Search still validates the caller's page size against the result window, then scroll capture uses the configured result window capped by `MAX_SCROLL_RETAINED_HITS`. Runtime cursor state stores only hits after the first page and enforces a context count, TTL, and byte budget tied to `memory_limit_bytes` capped at 32 MiB.

`delete_by_query` now uses a `RequireQuery` mode and returns a validation error when the body has no `query`. `update_by_query` keeps the migration-friendly `match_all` default, but it validates the saved-object namespace/workspace removal script before scanning documents. The recognized script shape is exact after whitespace and quote normalization, and params are resolved before mutation planning.

Reindex now parses `dest.op_type`. The default remains index/overwrite behavior, while `op_type=create` uses `WriteOperation::CreateDocument`, reports version conflicts, and honors `conflicts=proceed` without overwriting existing destination docs.

By-query and reindex planning run through `Store::apply_dynamic_write_operations_atomic`. That storage path builds write operations while holding the store write lock, validates mutations against one candidate database, and commits the resulting mutation transaction once. This removes the stale read-snapshot window and avoids cloning a fresh candidate database for each matched document.

Term query evaluation now treats array fields as matching when any element equals the scalar expected value. That makes Dashboards-style workspace filters work against saved-object documents whose `workspaces` or `namespaces` fields are arrays.

## Why This Works

The root cause was missing validation at API boundaries that matter for local OpenSearch compatibility: route shape, query intent, script safety, create semantics, scroll resource admission, and atomic mutation planning. The fix moves validation before mutation or retention, and routes accepted work through shared query and storage primitives so behavior is deterministic, bounded, and closer to OpenSearch client expectations.

This also keeps runtime agent fallback out of the story. Known mutating or wrong-shape APIs are classified deterministically before fallback can be considered, so a configured fallback agent cannot synthesize a write response for a known unsupported mutation shape.

## Prevention

- Keep route inventory tests for exact by-query shapes and path-form scroll. Global by-query mutation paths, mutating `POST` requests with extra path segments, and wrong methods on exact by-query routes should stay unsupported and non-fallback-eligible.
- Keep Dashboards migration fixture tests around scroll paging, reindex task polling, synchronous reindex responses, `op_type=create`, missing delete queries, workspace array terms, and near-miss scripts.
- For new mutating OpenSearch APIs, classify exact supported shapes first and make global, extra path, wrong method, or unknown script forms structured unsupported/errors before handler dispatch.
- For read-then-write APIs, build operations under the store write path instead of collecting matches from a separate read snapshot.
- For compatibility scripts, prefer named recognizers with source-traceable fixtures over substring checks. A near-miss script should fail before scanning documents.

## Related Issues

- [OpenSearch Lite P1 Code Review Hardening](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/solutions/security-issues/opensearch-lite-p1-code-review-hardening-2026-04-29.md:1) has moderate overlap around route classification, fallback safety, and storage validation. This document covers the later Dashboards migration API tranche.
- [OpenSearch Lite Kubernetes Workgroup Security](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/solutions/security-issues/opensearch-lite-kubernetes-workgroup-security-2026-04-30.md:1) is adjacent because it also treats route inventory and authorization as security boundaries.
- [OpenSearch Lite Snapshot Reserved Selector Delete Hardening](../security-issues/opensearch-lite-snapshot-reserved-selector-delete-hardening-2026-05-04.md) is adjacent because it applies exact-shape fail-closed handling to snapshot selector/control-token paths.
- [Dashboards gap analysis](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/opensearch-dashboards-gap-analysis.md:89) tracks the application-driven API surface that motivated these migration handlers.
- [Supported APIs](/Users/kiyu.gabriel/Development/cqlite-server/opensearch-lite/docs/supported-apis.md:39) documents the deterministic API surface after this tranche.
- No related GitHub issues were found by `gh issue list` for this tranche.
