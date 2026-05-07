---
title: mainstack-search Snapshot Route And Restore Parser Hardening
date: 2026-05-04
last_updated: 2026-05-06
category: security-issues
module: mainstack-search snapshot APIs
problem_type: security_issue
component: service_object
symptoms:
  - "`PUT /_snapshot/_all` and `PUT /_snapshot/local/all` could be blocked while `DELETE` still expanded those tokens destructively"
  - "`DELETE /_snapshot/_all` and `DELETE /_snapshot/local/_all` could delete every repository or snapshot through shared selector expansion"
  - "`POST /_snapshot/local/_restore` could be classified as snapshot creation instead of an unsupported restore-family route"
  - "`/%5Fsnapshot/local/snap-1/_restore` could classify as implemented/admin but skip restore parser validation"
  - "Explicit empty restore selectors such as indices:\"\" or indices:[] could widen to all indices"
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - testing_framework
  - documentation
tags:
  - mainstack-search
  - snapshot-api
  - route-safety
  - selector-expansion
  - restore-parser
  - write-safety
  - fail-closed
  - encoded-paths
---

# mainstack-search Snapshot Route And Restore Parser Hardening

## Problem

mainstack-search needed to reserve `_all` and `all` as snapshot selector tokens
rather than literal repository or snapshot names. The first fix rejected those
names during create operations, but destructive delete paths still reused the
same selector expansion helper as read/list APIs, so reserved tokens could still
fan out to every repository or snapshot.

The same route-shape issue appeared again with malformed snapshot operation
tokens: `POST /_snapshot/local/_restore` and `PUT /_snapshot/local/_clone` could
be treated as generic snapshot create requests unless the classifier reserved
those operation-token shapes before the name-slot route.

A later restore-parser tranche exposed the same family of boundary bugs in a
narrower form. `snapshot.restore` was intentionally promoted to implemented/admin
for authorization and parser routing while execution still returned unsupported.
That made parser bypasses and over-broad request interpretation matter before
the restore executor existed: encoded `/%5Fsnapshot/...` paths could be
classified as implemented/admin but miss the raw `_snapshot` handler dispatch,
and explicit empty restore selectors such as `indices: ""` or `indices: []`
collapsed into all-index restore intent.

## Symptoms

- `PUT /_snapshot/_all` and `PUT /_snapshot/local/all` returned validation
  errors, but `DELETE /_snapshot/_all` still returned success.
- `delete_repository` and `delete_snapshot` called `expand_names`, whose
  reserved-token branch expands `_all`, `all`, or `*` to every available name.
- Underscore-prefixed snapshot operation tokens such as `_restore` or `_clone`
  could still reach the generic snapshot create route when they appeared in the
  snapshot-name slot.
- The regression test checked `PUT` rejection and `GET` selector expansion, but
  it did not assert that rejected `DELETE` calls preserve existing data.
- Encoded namespace requests such as `/%5Fsnapshot/local/snap-1/_restore` were
  decoded by route classification, but handler dispatch still matched only the
  raw `_snapshot` segment.
- The first restore parser treated a missing `indices` field and an explicitly
  empty `indices` field the same, returning `RestoreIndices::All` for both.
- Timeout hardening initially rejected OpenSearch-shaped values such as `0`,
  `-1`, `10micros`, and `10nanos`.

## What Didn't Work

- Adding `_all` and `all` to `validate_name` was necessary but incomplete. That
  helper only protects paths that actually call literal-name validation.
- Reusing `expand_names` for both reads and deletes blurred two different
  policies: read/list APIs can accept wildcard selectors, while mutating APIs
  need exact caller intent before side effects.
- Testing only create and read behavior made the change look complete even
  though the destructive API family still had the original behavior.
- Covering only the canonical restore route was still too narrow. A later review
  showed malformed `_restore` and `_clone` paths in the snapshot-name slot could
  reach generic snapshot creation until route classification reserved them first
  (session history).
- During the restore parser tranche, treating "present but empty" as equivalent
  to "missing" was unsafe. Omitted `indices` is valid OpenSearch-shaped all-index
  intent, but an explicit empty string or array is ambiguous caller input that
  should not silently widen a future restore (session history).
- Route classification decoded path segments, but runtime handler dispatch did
  not consistently use that same decoded view. That let encoded snapshot
  namespace paths classify as implemented/admin while skipping restore parser
  validation (session history).
- Tightening timeout query parsing by hand caught malformed values but initially
  excluded valid OpenSearch client inputs such as `0`, `-1`, `10micros`, and
  `10nanos` (session history).

## Solution

Keep read/list selector expansion separate from mutating delete validation.
`get_repositories`, `get_snapshots`, and other list-style APIs still call
`expand_names`, so `_all`, `all`, and `*` remain useful OpenSearch-shaped
selectors. `delete_repository` and `delete_snapshot` now call an exact-name path
that validates each comma-separated target with `validate_name` before any
filesystem or manifest mutation.

```rust
pub fn delete_repository(&self, names: &str) -> StoreResult<Value> {
    self.ensure_persistent()?;
    let _lock = self.catalog_lock.lock().map_err(|_| lock_error())?;
    let names = exact_names(names, self.repository_names()?, "repository")?;
    for name in names {
        let dir = self.repository_dir(&name);
        if !dir.exists() {
            return Err(repository_missing(&name));
        }
        fs::remove_dir_all(&dir).map_err(io_error)?;
        sync_directory(self.root.as_ref()).map_err(io_error)?;
    }
    Ok(json!({ "acknowledged": true }))
}
```

The exact-name helper mirrors the non-wildcard branch of selector expansion but
never accepts selector tokens:

```rust
fn exact_names(raw: &str, available: Vec<String>, kind: &'static str) -> StoreResult<Vec<String>> {
    let requested = raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return Err(match kind {
            "repository" => invalid_repository(format!("invalid repository name [{raw}]")),
            _ => StoreError::new(
                400,
                "invalid_snapshot_name_exception",
                format!("invalid snapshot name [{raw}]"),
            ),
        });
    }

    let mut names = Vec::new();
    for name in requested {
        let name = validate_name(name, kind)?;
        if !available.contains(&name) {
            return Err(match kind {
                "repository" => repository_missing(&name),
                _ => snapshot_missing("", &name),
            });
        }
        names.push(name);
    }
    names.sort();
    names.dedup();
    Ok(names)
}
```

Snapshot names also reject a leading underscore. This matches OpenSearch's
snapshot-name validation and prevents future operation tokens from falling
through generic snapshot-name paths:

```rust
if kind == "snapshot" && name.starts_with('_') {
    return Err(StoreError::new(
        400,
        "invalid_snapshot_name_exception",
        format!("invalid snapshot name [{raw}]"),
    ));
}
```

Route classification handles malformed operation-token shapes before the generic
snapshot create/get/delete arm:

```rust
["_snapshot", _, "_restore"] => route("snapshot.restore", Tier::Unsupported, AccessClass::Admin),
["_snapshot", _, "_clone"] => route("snapshot.clone", Tier::Unsupported, AccessClass::Admin),
```

The restore parser tranche extends the same fail-closed boundary to parser-only
restore support. `POST /_snapshot/{repository}/{snapshot}/_restore` is
implemented/admin for authorization and deterministic routing, then
`parse_restore_request` validates the body and query before execution returns
unsupported. The missing-vs-empty distinction is explicit:

```rust
match body.get("indices") {
    None => Ok(RestoreIndices::All),
    Some(Value::String(raw)) => {
        let names = split_csv(raw);
        if names.is_empty() {
            Err(empty_indices_error())
        } else if names.iter().any(|name| matches!(name.as_str(), "_all" | "*" | "all")) {
            Ok(RestoreIndices::All)
        } else {
            Ok(RestoreIndices::Names(names))
        }
    }
    Some(Value::Array(values)) => {
        if values.is_empty() {
            return Err(empty_indices_error());
        }
        let mut names = Vec::new();
        for value in values {
            let Some(value) = value.as_str() else {
                return Err(StoreError::new(
                    400,
                    "parse_exception",
                    "snapshot restore indices array must contain only strings",
                ));
            };
            names.extend(split_csv(value));
        }
        if names.is_empty() {
            Err(empty_indices_error())
        } else if names.iter().any(|name| matches!(name.as_str(), "_all" | "*" | "all")) {
            Ok(RestoreIndices::All)
        } else {
            names.sort();
            names.dedup();
            Ok(RestoreIndices::Names(names))
        }
    }
    Some(_) => Err(StoreError::new(
        400,
        "parse_exception",
        "snapshot restore indices must be a string or array of strings",
    )),
}
```

Handler dispatch now uses the same decoded snapshot namespace view as route
classification, so encoded namespace paths enter the same restore parser path:

```rust
if parts
    .first()
    .is_some_and(|part| decode_path_param(part) == "_snapshot")
{
    return handle_snapshot(&state, &request, &parts).await;
}
```

The parser also accepts OpenSearch-shaped timeout syntax before returning the
fail-closed unsupported restore response:

```rust
if matches!(raw, "0" | "-1") {
    return Ok(());
}

match unit {
    "" | "nanos" | "micros" | "ms" | "s" | "m" | "h" | "d" => Ok(()),
    _ => Err(StoreError::new(
        400,
        "parse_exception",
        format!("unsupported time unit [{unit}] for query parameter [{key}]"),
    )),
}
```

The regression now exercises the full policy boundary:

- `PUT /_snapshot/_all` and `PUT /_snapshot/local/all` fail validation.
- `PUT /_snapshot/local/_hidden` fails validation because snapshot names may not
  start with `_`.
- `GET /_snapshot/_all` and `GET /_snapshot/local/_all` still expand as read
  selectors.
- `DELETE /_snapshot/_all`, `DELETE /_snapshot/all`, encoded reserved tokens,
  and mixed lists such as `local,_all` fail validation.
- `POST /_snapshot/local/_restore`, encoded `_restore`, `PUT
  /_snapshot/local/_clone`, and encoded `_clone` return unsupported without
  creating snapshots.
- `POST /%5Fsnapshot/local/snap-1/_restore` reaches restore parser validation
  instead of skipping the raw `_snapshot` handler branch.
- `indices: ""`, `indices: []`, and arrays that split to no names return
  `400 parse_exception`; omitted `indices` remains the only implicit all-index
  restore selector.
- `master_timeout` and `cluster_manager_timeout` accept OpenSearch-shaped
  no-op values such as `0`, `-1`, `10micros`, and `10nanos`, while malformed or
  duplicate values still fail before execution.
- After each rejected delete request, the test re-reads the existing repository
  or snapshot to prove no broad mutation happened.

## Why This Works

For the original delete-selector bug, the root cause was policy conflation
rather than missing path decoding. The HTTP handler already percent-decodes path
parameters before calling the snapshot service, so encoded tokens such as
`%5Fall` become `_all` at the service boundary. The unsafe part was that delete
operations interpreted those decoded tokens through the same wildcard expansion
path as reads.

Splitting exact-name validation from selector expansion makes caller intent
method-aware. Read/list APIs can continue to be permissive selectors, while
destructive APIs must name concrete repositories or snapshots and must pass the
same reserved-name validation as creation.

Reserving malformed `_restore` and `_clone` route shapes before the generic
snapshot-name arm keeps unsupported operation tokens in the admin/fail-closed
path. Rejecting underscore-prefixed snapshot names gives the service layer a
second guardrail if future routes accidentally pass operation tokens through
literal-name validation.

The restore parser fixes apply the same fail-closed principle to future restore
execution. An omitted `indices` field is an intentional all-index restore
request, but an explicitly empty selector is ambiguous and now fails before it
can become a broad mutation. The encoded namespace bug was a classifier/handler
normalization mismatch: classification decoded `/%5Fsnapshot`, while handler
dispatch still checked the raw path segment. Decoding the namespace before
dispatch keeps classifier, authorization, and handler behavior aligned for
encoded control paths. Accepting OpenSearch timeout syntax preserves client
compatibility without weakening the execution boundary, because restore
execution still returns unsupported after validation.

## Prevention

- Treat route method and operation type as part of the validation policy. A token
  that is valid for `GET` list expansion is not automatically valid for `DELETE`.
- When adding reserved-name validation, search for every helper that can
  interpret those names, not just create/update paths.
- Reserve operation-token shapes in the route classifier before generic
  name-slot arms, and include percent-encoded token forms in the regression.
- Keep route classification and runtime handler dispatch on the same decoded
  path model for snapshot/control namespaces.
- Treat missing and empty request fields as distinct states when the field can
  widen mutation scope. Omitted `indices` can mean all; explicitly empty
  `indices` should fail unless a no-op semantic is deliberately added.
- Validate common OpenSearch query syntax before rejecting unsupported execution,
  especially for compatibility parameters that official clients may send.
- For unsupported snapshot/control operation tokens, inventory tests should
  assert both `Tier::Unsupported` and `AccessClass::Admin` so the route cannot
  drift into runtime fallback eligibility.
- Regression tests for mutating selector boundaries should assert both the error
  response and the absence of side effects.
- Include encoded namespace, encoded operation-token, mixed-list, and
  present-but-empty forms in snapshot tests because the handler and classifier
  both decode path parameters before service validation.

## Related Issues

- [mainstack-search P1 Code Review Hardening](mainstack-search-p1-code-review-hardening-2026-04-29.md)
- [mainstack-search Agent Write Fallback And Durable Replay Hardening](mainstack-search-agent-write-fallback-durable-replay-hardening-2026-04-30.md)
- [mainstack-search Kubernetes Workgroup Security](mainstack-search-kubernetes-workgroup-security-2026-04-30.md)
- [mainstack-search Dashboards Migration API Surface Hardening](../integration-issues/mainstack-search-dashboards-migration-api-surface-hardening-2026-04-30.md)
