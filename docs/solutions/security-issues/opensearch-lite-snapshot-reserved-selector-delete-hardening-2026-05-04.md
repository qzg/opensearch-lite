---
title: OpenSearch Lite Snapshot Reserved Selector Delete Hardening
date: 2026-05-04
category: security-issues
module: opensearch-lite snapshot APIs
problem_type: security_issue
component: service_object
symptoms:
  - "`PUT /_snapshot/_all` and `PUT /_snapshot/local/all` could be blocked while `DELETE` still expanded those tokens destructively"
  - "`DELETE /_snapshot/_all` and `DELETE /_snapshot/local/_all` could delete every repository or snapshot through shared selector expansion"
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

## Symptoms

- `PUT /_snapshot/_all` and `PUT /_snapshot/local/all` returned validation
  errors, but `DELETE /_snapshot/_all` still returned success.
- `delete_repository` and `delete_snapshot` called `expand_names`, whose
  reserved-token branch expands `_all`, `all`, or `*` to every available name.
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

The regression now exercises the full policy boundary:

- `PUT /_snapshot/_all` and `PUT /_snapshot/local/all` fail validation.
- `GET /_snapshot/_all` and `GET /_snapshot/local/_all` still expand as read
  selectors.
- `DELETE /_snapshot/_all`, `DELETE /_snapshot/all`, encoded reserved tokens,
  and mixed lists such as `local,_all` fail validation.
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

## Prevention

- Treat route method and operation type as part of the validation policy. A token
  that is valid for `GET` list expansion is not automatically valid for `DELETE`.
- When adding reserved-name validation, search for every helper that can
  interpret those names, not just create/update paths.
- Regression tests for mutating selector boundaries should assert both the error
  response and the absence of side effects.
- Include encoded and mixed-list forms in destructive-route tests because the
  handler decodes path parameters before service validation.

## Related Issues

- [OpenSearch Lite P1 Code Review Hardening](opensearch-lite-p1-code-review-hardening-2026-04-29.md)
- [OpenSearch Lite Agent Write Fallback And Durable Replay Hardening](opensearch-lite-agent-write-fallback-durable-replay-hardening-2026-04-30.md)
