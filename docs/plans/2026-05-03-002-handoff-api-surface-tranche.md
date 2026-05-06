---
title: "handoff: API Surface Tranche Progress"
type: handoff
status: completed
date: 2026-05-03
completed: 2026-05-06
---

# handoff: API Surface Tranche Progress

## Current State

This handoff is complete and has been superseded by committed follow-up work on
`main`. The implementation progress through the original handoff landed in:

- `7a51b2c fix(api): harden snapshot and cursor edge cases`

The rename and later safety follow-ups now live on `main`; this file is kept as
historical context for the API surface tranche.

## What Landed

- Snapshot repository APIs now fail closed under `--ephemeral` instead of writing
  repository metadata or blobs under the configured data directory.
- Snapshot status routes such as `/_snapshot/_status`,
  `/_snapshot/{repo}/_status`, and deeper status paths classify as unsupported
  admin APIs before generic snapshot repository/snapshot route arms.
- Route classification and the authz control fallback guard now percent-decode
  path segments before checking reserved namespaces, closing encoded
  `_snapshot`, `_security`, `_tasks`, and `_task` fallback gaps.
- Non-PIT sorted `_search` now appends a deterministic `_shard_doc` tie-breaker
  when the caller did not already include `_id` or `_shard_doc`, so
  `search_after` no longer skips equal-sort hits.
- PIT runtime retained databases use shared `Arc<Database>` references, reducing
  cloned retained snapshot pressure and keeping expiry/list/search budget tests
  explicit.
- Release packaging now includes the main support/security/fallback docs in the
  generated archive.
- Docs now state the durable-only snapshot behavior, snapshot status unsupported
  boundary, and deterministic cursor behavior.

## Verification

Passed:

```sh
cargo fmt --check
git diff --check
cargo clippy --all-targets -- -D warnings
cargo test --test api_inventory --test security_surface --test snapshot_surface --test search_surface
CARGO_BUILD_JOBS=1 cargo test
```

The default parallel `cargo test` was terminated during concurrent rustc test
compilation with SIGTERM. Re-running with `CARGO_BUILD_JOBS=1` completed the full
suite successfully.

## Completed Follow-Up: Reserved Snapshot Names

The prior P2 recommendation below has been implemented and covered by
regression tests:

- Reject literal repository/snapshot names `_all` and `all` in
  `validate_name`.
- Keep `_all`, `all`, and `*` valid only as wildcard selector tokens in
  `expand_names`.
- Add a regression proving `PUT /_snapshot/_all` and
  `PUT /_snapshot/local/all` fail validation, while
  `GET /_snapshot/_all` and `GET /_snapshot/local/_all` still expand as
  selectors.

Additional destructive-route coverage now verifies rejected `DELETE` requests
preserve existing repositories/snapshots, encoded reserved tokens are rejected,
and malformed `_restore`/`_clone` operation tokens stay in unsupported admin
routes instead of generic snapshot creation. See:

- `tests/snapshot_surface.rs`
- `src/snapshots/service.rs`
- `docs/solutions/security-issues/mainstack-search-snapshot-reserved-selector-delete-hardening-2026-05-04.md`

## Suggested Next API Tranche

1. Use `docs/plans/2026-05-06-001-feat-snapshot-restore-plan.md` for the next
   restore-specific tranche.
2. Expand large-result compatibility only where callers need it:
   PIT plus `search_after` is now the preferred OpenSearch-shaped path; true
   HTTP response streaming should stay deferred unless a concrete caller needs a
   mainstack-search-specific export endpoint.
3. Continue API inventory visualization/coverage work by grouping unsupported
   APIs into:
   control/admin safety boundaries, distributed-cluster features, plugin surfaces,
   Lucene/search parity gaps, and unsupported mutation families.

## Useful Entry Points

- `src/api_spec/mod.rs`: route classification and fail-closed API family guards.
- `src/security/authz.rs`: fallback authorization boundary for control-like
  namespaces.
- `src/snapshots/service.rs`: local snapshot repository catalog and validation.
- `src/search/evaluator.rs`: sort values, tie-breakers, and `search_after`.
- `src/runtime.rs`: process-local scroll, task, and PIT runtime state.
- `tests/api_inventory.rs`: classification and inventory regressions.
- `tests/snapshot_surface.rs`: snapshot repository behavior.
- `tests/search_surface.rs` and `tests/pit_surface.rs`: pagination and PIT
  regressions.
