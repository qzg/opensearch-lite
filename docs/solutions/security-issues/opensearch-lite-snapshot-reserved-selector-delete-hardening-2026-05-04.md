---
title: OpenSearch Lite Snapshot Reserved Selector Delete Hardening
date: 2026-05-04
last_updated: 2026-05-04
category: security-issues
module: opensearch-lite snapshot APIs
problem_type: security_issue
component: service_object
symptoms:
  - "`PUT /_snapshot/_all` and `PUT /_snapshot/local/all` could be blocked while `DELETE` still expanded those tokens destructively"
  - "`DELETE /_snapshot/_all` and `DELETE /_snapshot/local/_all` could delete every repository or snapshot through shared selector expansion"
  - "`POST /_snapshot/local/_restore` could be classified as snapshot creation instead of an unsupported restore-family route"
  - "The initial regression covered create rejection and read selector expansion but missed destructive delete paths"
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - testing_framework
  - documentation
tags:
  - opensearch-lite
  - snapshot-api
  - route-safety
  - selector-expansion
  - write-safety
  - fail-closed
---

# OpenSearch Lite Snapshot Reserved Selector Delete Hardening

## Problem

OpenSearch Lite needed to reserve `_all` and `all` as snapshot selector tokens
rather than literal repository or snapshot names. The first fix rejected those
names during create operations, but destructive delete paths still reused the
same selector expansion helper as read/list APIs, so reserved tokens could still
fan out to every repository or snapshot.

The same route-shape issue appeared again with malformed snapshot operation
tokens: `POST /_snapshot/local/_restore` and `PUT /_snapshot/local/_clone` could
be treated as generic snapshot create requests unless the classifier reserved
those operation-token shapes before the name-slot route.

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
- After each rejected delete request, the test re-reads the existing repository
  or snapshot to prove no broad mutation happened.

## Why This Works

The root cause was a policy conflation, not path decoding. The HTTP handler
already percent-decodes path parameters before calling the snapshot service, so
encoded tokens such as `%5Fall` become `_all` at the service boundary. The unsafe
part was that delete operations interpreted those decoded tokens through the same
wildcard expansion path as reads.

Splitting exact-name validation from selector expansion makes caller intent
method-aware. Read/list APIs can continue to be permissive selectors, while
destructive APIs must name concrete repositories or snapshots and must pass the
same reserved-name validation as creation.

Reserving malformed `_restore` and `_clone` route shapes before the generic
snapshot-name arm keeps unsupported operation tokens in the admin/fail-closed
path. Rejecting underscore-prefixed snapshot names gives the service layer a
second guardrail if future routes accidentally pass operation tokens through
literal-name validation.

## Prevention

- Treat route method and operation type as part of the validation policy. A token
  that is valid for `GET` list expansion is not automatically valid for `DELETE`.
- When adding reserved-name validation, search for every helper that can
  interpret those names, not just create/update paths.
- Reserve operation-token shapes in the route classifier before generic
  name-slot arms, and include percent-encoded token forms in the regression.
- For unsupported snapshot/control operation tokens, inventory tests should
  assert both `Tier::Unsupported` and `AccessClass::Admin` so the route cannot
  drift into runtime fallback eligibility.
- Regression tests for mutating selector boundaries should assert both the error
  response and the absence of side effects.
- Include encoded and mixed-list forms in destructive-route tests because the
  handler decodes path parameters before service validation.

## Related Issues

- [OpenSearch Lite P1 Code Review Hardening](opensearch-lite-p1-code-review-hardening-2026-04-29.md)
- [OpenSearch Lite Agent Write Fallback And Durable Replay Hardening](opensearch-lite-agent-write-fallback-durable-replay-hardening-2026-04-30.md)
- [OpenSearch Lite Kubernetes Workgroup Security](opensearch-lite-kubernetes-workgroup-security-2026-04-30.md)
- [OpenSearch Lite Dashboards Migration API Surface Hardening](../integration-issues/opensearch-lite-dashboards-migration-api-surface-hardening-2026-04-30.md)
