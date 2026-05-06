---
title: "feat: Add Snapshot Restore"
type: feat
status: active
date: 2026-05-06
origin: docs/plans/2026-05-03-001-feat-snapshot-pit-pagination-plan.md
---

# feat: Add Snapshot Restore

## Summary

Add a narrow, durable, OpenSearch-shaped snapshot restore path for native
mainstack-search repositories. The first restore tranche should restore selected
indexes from a local snapshot into the live store atomically, support index
renaming and alias inclusion, reject unsupported global/remote/partial restore
features explicitly, and preserve the current fail-closed route and fallback
boundaries.

This is not full OpenSearch restore parity. It is a local development recovery
and migration primitive for snapshots created by mainstack-search itself.

## Problem Frame

Snapshot repository create/get/delete/verify/cleanup now works in durable mode,
but `POST /_snapshot/{repository}/{snapshot}/_restore` still returns a
fail-closed unsupported response. That is honest, but it leaves native snapshot
archives one-way: a developer can create and inspect local snapshots but cannot
restore them into a clean or renamed local state.

Restore is the riskiest remaining snapshot operation because it mutates live
state from repository files. It needs stronger planning than the repository
management subset: candidate validation before mutation, one durable replay
boundary, explicit unsupported-option handling, and tests that prove failed
restore requests leave the live store unchanged.

## Requirements

- R1. Restore remains durable-mode only; `--ephemeral` continues to fail closed
  for snapshot APIs.
- R2. Restore is admin-only and must not route through runtime agent fallback.
- R3. Restore only supports snapshots created by the native
  `SnapshotService` repository format under `--data-dir/repositories`.
- R4. Restore validates the request and repository blob into a candidate
  `Database` before mutating live state.
- R5. Restore commits through one durable boundary that replays correctly after
  restart and crash windows.
- R6. Restore supports selected index restore with exact names, `_all`/`all`/`*`,
  simple wildcard includes, and explicit not-found behavior.
- R7. Restore supports `rename_pattern` plus `rename_replacement` for target
  index names and rejects target-name collisions before mutation.
- R8. Restore supports `include_aliases` for aliases whose target indexes are
  restored; alias targets must follow index renames.
- R9. Restore rejects unsupported options such as `include_global_state: true`,
  `storage_type: remote_snapshot`, remote-store repository overrides, alias
  rename fields, and partial/corrupt snapshot states with OpenSearch-shaped
  errors.
- R10. Restore invalidates or documents runtime contexts deliberately: for this
  tranche, successful restore should clear scroll and PIT contexts so callers do
  not accidentally continue from pre-restore state.
- R11. Docs and generated coverage identify restore as implemented only for the
  narrow native local subset and keep restore options outside that subset listed
  as unsupported.

## Scope Boundaries

- Do not implement remote repositories, searchable snapshots, remote-backed
  storage restore, clone snapshot, restore status polling, or distributed shard
  recovery.
- Do not implement OpenSearch Security plugin snapshot/global-state behavior.
  `include_global_state: true` remains unsupported.
- Do not restore snapshots produced by full OpenSearch; native repository blobs
  store serialized mainstack-search `Database` state.
- Do not add asynchronous restore tasks in the first tranche. Treat
  `wait_for_completion` as a response-shape option for a synchronous operation.
- Do not support restoring into open existing indexes. Since mainstack-search
  does not model close/open index state, callers must delete or rename targets.
- Do not preserve old PIT or scroll contexts across restore completion.

## Context And Current Code

- `src/api/mod.rs` already routes `_snapshot` traffic to `handle_snapshot` before
  fallback and currently returns unsupported for unmatched restore paths.
- `src/snapshots/service.rs` owns repository validation, generation files,
  snapshot manifests, blob reads/writes, exact-name delete validation, and
  read/list selector expansion.
- `tests/snapshot_surface.rs` already proves restore fails closed without
  mutation and proves reserved selector tokens cannot become destructive names.
- `src/storage/mod.rs` owns the only safe durable mutation boundary:
  `commit_lock`, candidate `Database` validation, mutation log transaction
  begin/commit, memory checks, and dirty snapshot scheduling.
- `src/storage/mutation_log.rs` replays transaction records and is the right
  versioned place to introduce a dedicated restore mutation record.
- `src/runtime.rs` owns process-local scroll, task, and PIT registries; restore
  should clear restore-sensitive runtime contexts through explicit runtime APIs.

## External Reference Notes

OpenSearch documents restore as `POST /_snapshot/{repository}/{snapshot}/_restore`
with `wait_for_completion`, optional `indices`, `ignore_unavailable`,
`include_aliases`, `include_global_state`, `partial`, `rename_pattern`,
`rename_replacement`, `index_settings`, `ignore_index_settings`, remote-store
fields, and `storage_type`. It also states existing open indexes with the same
name must be closed, deleted, or renamed first.

The compatibility target for this plan is intentionally narrower:
mainstack-search supports local indexes and aliases, has no close-index state,
and stores native snapshot blobs rather than Lucene shard snapshots.

The nearby `../OpenSearch` checkout referenced in `AGENTS.md` was not present in
this workspace during planning, so official OpenSearch docs were used as the
external reference.

## Key Technical Decisions

- Use a dedicated restore mutation rather than expanding every restored document
  into thousands of individual mutation-log entries. A single versioned restore
  record is easier to replay atomically, keeps large restores readable as one
  operation, and avoids creating a transaction begin line with an enormous
  document-mutation array.
- Build the candidate live `Database` under `Store` ownership, not inside
  `SnapshotService`. The snapshot service can load repository data and blobs,
  but only the store should validate limits and publish durable mutations.
- Preserve snapshot document versions and sources from the blob, then set the
  live database `seq_no` to at least the max restored/live document sequence so
  later writes remain monotonic.
- Treat existing target indexes as conflicts. mainstack-search does not model
  closed indexes, so the safe local alternatives are delete first or use
  `rename_pattern`/`rename_replacement`.
- Apply index renames before alias restoration. Aliases whose source targets are
  restored should point at the renamed target index; aliases that would conflict
  with existing indexes or unrelated live aliases should fail the restore before
  mutation.
- Clear PIT and scroll contexts after successful restore. This is stricter than
  letting frozen contexts live, but it avoids confusing local callers after a
  destructive admin operation and is easier to explain in docs.

## Implementation Units

- U1. **Restore Route And Request Classification**

**Goal:** Promote the exact restore route from unsupported to deterministic
admin behavior while preserving fail-closed handling for malformed snapshot
operation tokens.

**Requirements:** R1, R2, R9, R11

**Files:**
- Modify: `src/api_spec/mod.rs`
- Modify: `build.rs`
- Modify: `src/api/mod.rs`
- Test: `tests/api_inventory.rs`
- Test: `tests/security_surface.rs`
- Test: `tests/snapshot_surface.rs`

**Approach:** Add exact route inventory/classification for
`POST /_snapshot/{repository}/{snapshot}/_restore` as implemented/admin. Keep
`/_snapshot/{repository}/_restore`, encoded operation tokens in name slots,
wrong methods, clone/status, and extra path segments unsupported/admin. Parse
`wait_for_completion` but keep execution synchronous in this tranche.

**Test scenarios:**
- Happy path: the exact restore route classifies as implemented/admin.
- Error path: read-only users are rejected before handler execution.
- Error path: malformed `_restore` or `_clone` name-slot paths remain
  unsupported and do not create snapshots.
- Error path: unsupported restore extra segments do not enter fallback.

**Verification:** route inventory, runtime classification, and authz agree.

- U2. **Restore Request Parser And Selection Policy**

**Goal:** Convert restore bodies into a validated local restore plan before any
repository blob or live store mutation is attempted.

**Requirements:** R4, R6, R7, R8, R9

**Files:**
- Modify: `src/snapshots/service.rs`
- Create or modify: `src/snapshots/restore.rs`
- Test: `tests/snapshot_surface.rs`

**Approach:** Parse a small typed request: `indices`, `ignore_unavailable`,
`include_aliases`, `include_global_state`, `partial`, `rename_pattern`,
`rename_replacement`, `index_settings`, and `ignore_index_settings`. Support
exact names, `_all`/`all`/`*`, and simple wildcard includes over the snapshot
manifest's indices. Reject unsupported fields explicitly, including
`include_global_state: true`, remote-store fields, `storage_type` values other
than `local`, alias rename fields, and invalid body types. Validate rename
regex/replacement once, then compute target index names and detect collisions.

**Test scenarios:**
- Happy path: no body restores all snapshot indices.
- Happy path: exact and wildcard `indices` restore the selected subset.
- Happy path: `rename_pattern`/`rename_replacement` computes unique targets.
- Error path: missing requested index errors unless `ignore_unavailable` is
  true.
- Error path: two source indexes renamed to one target fail before mutation.
- Error path: unsupported global/remote/alias-rename options return structured
  unsupported errors.

**Verification:** parser tests prove each accepted request maps to a concrete,
collision-free restore plan or returns before reading live mutable state.

- U3. **Repository Blob Load And Candidate Database Construction**

**Goal:** Load the snapshot blob and build a candidate restored subset that is
safe to hand to `Store`.

**Requirements:** R3, R4, R6, R7, R8, R9

**Files:**
- Modify: `src/snapshots/service.rs`
- Create or modify: `src/snapshots/restore.rs`
- Test: `tests/snapshot_surface.rs`

**Approach:** Add a `prepare_restore`-style service method that reads the
repository generation, finds the manifest, opens the referenced database blob,
deserializes it as `Database`, checks manifest/blob consistency, applies index
selection and renaming, filters or retargets aliases, applies supported
`index_settings`/`ignore_index_settings` rules, recomputes store sizes, and
returns a `PreparedSnapshotRestore` value. Reject corrupt, missing, or
non-success snapshots.

**Test scenarios:**
- Happy path: restored candidate includes mappings, settings, documents,
  tombstones, and selected aliases.
- Happy path: `include_aliases: false` restores indexes without aliases.
- Error path: missing blob, corrupt blob JSON, or manifest/blob mismatch returns
  repository/snapshot corruption without live mutation.
- Error path: attempts to change or ignore immutable shard settings are rejected.

**Verification:** the service can prepare restore candidates while live store
state remains unchanged.

- U4. **Atomic Store Restore Commit**

**Goal:** Publish the prepared restore through one durable, replayable store
boundary.

**Requirements:** R4, R5, R10

**Files:**
- Modify: `src/storage/mod.rs`
- Modify: `src/storage/mutation_log.rs`
- Modify: `src/runtime.rs`
- Test: `tests/snapshot_surface.rs`
- Test: `tests/durable_agent_read_surface.rs`
- Test: `tests/pit_surface.rs`

**Approach:** Add a `Mutation::RestoreSnapshot` variant carrying repository,
snapshot, restored index names, and a compact restored `Database` subset. Add a
store method that takes `PreparedSnapshotRestore`, acquires `commit_lock`, builds
a live candidate by inserting restored indexes/aliases into the current database,
rejects existing target index conflicts, validates index/document/memory limits,
updates sequence counters, appends one transaction containing the restore
mutation, publishes the candidate only after commit, schedules snapshot flush,
and clears PIT/scroll runtime contexts after success.

**Test scenarios:**
- Happy path: restore into an empty durable server, restart, and read restored
  documents.
- Happy path: restore with rename into a server that still has the original live
  index.
- Error path: target index conflict leaves live documents untouched.
- Error path: memory/document/index limit rejection leaves live documents and
  repository files untouched.
- Integration: restart after restore replays the restored state without requiring
  the repository blob to still be read.
- Integration: PIT/scroll contexts created before restore are missing or cleared
  after restore completion.

**Verification:** restore is atomic from API caller and replay perspectives.

- U5. **Restore Response Shape And Docs**

**Goal:** Return OpenSearch-shaped restore responses and document the local
subset precisely.

**Requirements:** R2, R9, R10, R11

**Files:**
- Modify: `src/api/mod.rs`
- Modify: `docs/supported-apis.md`
- Modify: `docs/compatibility.md`
- Modify: `docs/agent-fallback.md`
- Modify: `docs/api-coverage.md`
- Modify: `examples/api_coverage.rs`
- Test: `tests/api_inventory.rs`
- Test: `tests/snapshot_surface.rs`

**Approach:** For synchronous restore, return a compact body with snapshot name,
restored indices, and shard counts. If `wait_for_completion=false`, still finish
the restore before returning but use an accepted-style response only if tests
show official clients require it; otherwise prefer the completed response shape
because there is no task. Document unsupported options and state that restore
clears runtime cursor contexts.

**Test scenarios:**
- Happy path: response names restored indexes and successful shard counts.
- Error path: unsupported options include a recovery hint.
- Integration: generated coverage counts restore as deterministic implemented
  and leaves clone/status/remote/searchable snapshot APIs closed.

**Verification:** users can tell exactly what local restore can and cannot do.

## System-Wide Impact

- **Durability:** restore adds the first whole-index state mutation. Replay must
  not depend on repository files after commit.
- **Runtime state:** PIT and scroll registries hold old views; clearing them on
  success avoids stale reads after admin restore.
- **Resource limits:** restore can exceed index/document/memory limits in one
  operation; all limits must be checked on the candidate before publication.
- **Security:** restore remains admin-only and never fallback-eligible.
- **Docs:** restore support must be described as native local restore, not
  OpenSearch repository interoperability.

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| Restore partially mutates live state. | Candidate database plus one durable commit boundary; tests assert failed restores leave live data unchanged. |
| Replay requires repository blobs that may be deleted. | Dedicated restore mutation stores the restored subset needed for replay. |
| Rename collisions silently overwrite data. | Compute all target names before mutation and reject conflicts/collisions. |
| Alias restore points to wrong targets after rename. | Retarget aliases through the source-to-target index map and test renamed alias reads. |
| Unsupported OpenSearch options look accepted. | Typed parser rejects unsupported fields with structured errors and docs list them. |
| Runtime cursors read pre-restore state. | Clear PIT/scroll contexts after successful restore and document the behavior. |

## Phased Delivery

1. Route classification and request parser with tests, keeping handler
   unsupported until parser behavior is proven.
2. Repository blob load and candidate construction tests.
3. Store restore mutation and replay tests.
4. Handler response shape, runtime context clearing, docs, and coverage refresh.

## Open Questions For Implementation

- Should `wait_for_completion=false` return an accepted response with no task,
  or should synchronous local restore always return the completed snapshot body?
  Resolve with the official clients used in smoke tests before finalizing U5.
- Should `indices` support exclusion syntax such as `logs*,-logs-old` in V1?
  The plan allows exact and simple wildcard includes; exclusion support can be
  added if implementation stays small and testable.
- Should `index_settings` support deep dot-path removal/merge in V1, or should
  it be rejected except for a small allowlist? Prefer rejecting if the merge
  semantics become ambiguous.

## Sources And References

- Current snapshot service: `src/snapshots/service.rs`
- Current API handler: `src/api/mod.rs`
- Durable store boundary: `src/storage/mod.rs`
- Mutation log replay: `src/storage/mutation_log.rs`
- Snapshot tests: `tests/snapshot_surface.rs`
- Durable replay tests: `tests/durable_agent_read_surface.rs`
- PIT tests: `tests/pit_surface.rs`
- Prior tranche: `docs/plans/2026-05-03-001-feat-snapshot-pit-pagination-plan.md`
- Reserved-name learning:
  `docs/solutions/security-issues/mainstack-search-snapshot-reserved-selector-delete-hardening-2026-05-04.md`
- OpenSearch Restore Snapshot API:
  `https://docs.opensearch.org/latest/api-reference/snapshots/restore-snapshot/`
- OpenSearch snapshot restore guide:
  `https://docs.opensearch.org/2.11/tuning-your-cluster/availability-and-recovery/snapshots/snapshot-restore/`
