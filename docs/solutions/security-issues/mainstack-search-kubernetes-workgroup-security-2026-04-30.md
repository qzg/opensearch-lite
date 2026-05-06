---
title: mainstack-search Kubernetes Workgroup Security
date: 2026-04-30
category: security-issues
module: mainstack-search security
problem_type: security_issue
component: authentication
symptoms:
  - "Non-loopback container deployments could be started without TLS and authentication"
  - "Runtime agent fallback and unsupported control APIs needed to remain behind authentication and authorization"
  - "Mounted TLS and users-file mistakes needed shell-friendly diagnostics without exposing secrets"
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - tooling
  - testing_framework
  - documentation
tags:
  - mainstack-search
  - tls
  - basic-auth
  - kubernetes
  - docker
  - agent-fallback
  - rustls
  - security
---

# mainstack-search Kubernetes Workgroup Security

## Problem

mainstack-search needed a secured workgroup posture before it could be safely run in Docker or Kubernetes on a non-loopback interface. The local loopback experience still needed to stay frictionless, but container-style binding to `0.0.0.0` could not silently expose HTTP/no-auth APIs, runtime agent fallback, or local development data.

## Symptoms

- Docker and Kubernetes deployments naturally bind inside the container to a non-loopback address, which made insecure exposure easy to configure accidentally.
- Authentication had to work with official OpenSearch client shapes, but it also had to fail closed before body buffering, API dispatch, and runtime agent fallback.
- Read-only users needed to call OpenSearch read APIs that use `POST`, such as search and count, without granting broad write access.
- TLS material and users files are commonly mounted as Secrets, so startup diagnostics needed to identify missing, malformed, unreadable, or mismatched files without logging secret values.
- Coding agents needed commands that work through `docker exec` or `kubectl exec` to validate the mounted configuration and repair setup mistakes.

## What Didn't Work

- Treating non-loopback binding as merely "allowed" was too weak. Container deployments need an explicit distinction between "bind to a non-loopback address" and "allow insecure non-loopback HTTP/no-auth."
- Validating only that certificate and key files exist was insufficient. A mismatched cert/key pair can parse successfully but still fail when Rustls builds the server configuration.
- Putting authorization only in the HTTP router left a future internal caller of `api::handle_request` able to bypass the security check. The public handler boundary also needed to enforce authorization.
- Classifying access by HTTP method alone would have broken OpenSearch compatibility because several safe read APIs use `POST`.
- Reading password files verbatim in smoke tooling made mounted Secret files with a trailing newline fail authentication even though that is a normal Secret shape.

## Solution

The fix created a pre-dispatch security layer and made deployment posture explicit. Loopback-only local development still defaults to HTTP/no-auth, but non-loopback exposure now requires both TLS and a users file unless the operator passes `--allow-insecure-non-loopback` as an explicit development exception.

```rust
if !allow_nonlocal_listen || listen.ip().is_loopback() {
    return Ok(());
}

if self.allow_insecure_non_loopback {
    return Ok(());
}

match (self.tls.is_some(), self.users_file.is_some()) {
    (true, true) => Ok(()),
    (true, false) => Err("--listen is non-loopback; configure --users-file with TLS ...".to_string()),
    (false, true) => Err("--listen is non-loopback; configure TLS ...".to_string()),
    (false, false) => Err("--listen is non-loopback; configure TLS and --users-file ...".to_string()),
}
```

The TLS path uses Rustls through `axum-server`, with PEM certificate, key, optional server CA, and optional client-certificate CA inputs loaded from files. `--validate-config` now calls both the lightweight PEM parser and the Rustls config builder, so cert/key mismatches fail during diagnostics rather than first request handling.

Authentication is an Axum middleware that inspects the raw `HeaderMap` before request normalization. It rejects missing, duplicate, non-UTF-8, non-Basic, malformed, empty-username, and invalid credentials with OpenSearch-shaped `401 security_exception` responses and `WWW-Authenticate`. Invalid username/password attempts are delayed by `--auth-failure-delay-ms` to provide a simple online guessing control.

Authorization is route-inventory based instead of method-only. `build.rs` generates route access classes, `src/api_spec/mod.rs` carries `AccessClass`, and `src/security/authz.rs` maps authenticated principals to read, write, or admin permissions.

```rust
let allowed = match route.access {
    AccessClass::Read => request.security.can_read(),
    AccessClass::Write => request.security.can_write(),
    AccessClass::Admin => request.security.is_admin(),
};
```

The router authorizes before dispatch, and `api::handle_request` authorizes as well so the API boundary remains guarded if future code calls it directly. Runtime agent fallback stays behind the same authentication and authorization checks, redacts secret-like query/body fields, and fails closed for security/control namespaces such as `_plugins/_security`, `_opendistro/_security`, `_security`, `_snapshot`, `_tasks`, and `_task`.

The user-facing workflow is documented and testable from a shell. `docs/security.md` covers the users-file schema, TLS flags, role model, and validation commands. `docs/kubernetes-security.md` shows mounted Secret layouts, TCP probes, restart-based Secret rotation, and `kubectl exec` validation. The Docker assets include a non-root image path and a secure compose example with mounted TLS/users files.

The official-client smoke scripts now support secure mode through `MAINSTACK_SEARCH_SECURE_SMOKE=1`, generate temporary test-only certs and users files, and configure Python, JavaScript, and Java clients for HTTPS, Basic auth, and CA trust. External smoke runs also support password files and strip trailing CR/LF from mounted secrets.

## Why This Works

The root cause was missing validation at the deployment and request trust boundaries. A local OpenSearch-compatible server can be intentionally permissive on loopback, but once it binds to a non-loopback interface it needs to fail closed before accepting traffic.

The split posture keeps the common cases distinct:

- Loopback development remains simple and disposable.
- Workgroup and Kubernetes deployments require in-process TLS and authentication.
- Insecure non-loopback development remains possible, but only through a visibly named exception flag.

The request path also now has one consistent security story. Authentication happens before large body handling and API dispatch. Authorization uses the generated route inventory, which preserves OpenSearch read APIs that use `POST` while denying writes for read-only users. Runtime agent fallback cannot become a side channel because it is authorized before invocation and denied for local security/control namespaces.

## Prevention

- Keep `tests/security_surface.rs` around the posture and request-boundary behavior: non-loopback requires TLS/auth, direct `AppState` construction cannot bypass posture validation, read-only users can call read `POST` APIs, mutations are denied, and security/control paths do not enter fallback.
- Keep `tests/tls_surface.rs` around real HTTPS listener behavior and `--validate-config` diagnostics, including cert/key mismatch rejection.
- Preserve `tests/api_inventory.rs` access-class checks when adding new OpenSearch API coverage. New read `POST` APIs should be marked read intentionally, not inferred from method alone.
- Run the secure official-client smokes after changing TLS, auth, or users-file behavior:

```sh
MAINSTACK_SEARCH_SECURE_SMOKE=1 scripts/run-python-client-smoke.sh
MAINSTACK_SEARCH_SECURE_SMOKE=1 scripts/run-javascript-client-smoke.sh
MAINSTACK_SEARCH_SECURE_SMOKE=1 scripts/run-java-client-smoke.sh
```

- Keep docs and examples secret-file oriented. Mounted files and env vars are easier for Docker, Kubernetes, and coding agents to inspect and repair than secrets embedded in command-line arguments.

## Related Issues

- [mainstack-search P1 Code Review Hardening](/home/kiyu/Development/IBM/mainstack-search/docs/solutions/security-issues/mainstack-search-p1-code-review-hardening-2026-04-29.md:1) has moderate overlap: both documents treat route classification and agent fallback as security boundaries, but this doc covers the deployment posture, TLS/auth, and shell-operable diagnostics tranche.
- [mainstack-search Snapshot Reserved Selector Delete Hardening](mainstack-search-snapshot-reserved-selector-delete-hardening-2026-05-04.md) applies the same route-inventory and admin fail-closed boundary to snapshot selector/control-token APIs.
- [docs/security.md](/home/kiyu/Development/IBM/mainstack-search/docs/security.md:1)
- [docs/kubernetes-security.md](/home/kiyu/Development/IBM/mainstack-search/docs/kubernetes-security.md:1)
- [docs/agent-fallback.md](/home/kiyu/Development/IBM/mainstack-search/docs/agent-fallback.md:1)
- [docs/plans/2026-04-30-001-feat-kubernetes-workgroup-security-plan.md](/home/kiyu/Development/IBM/mainstack-search/docs/plans/2026-04-30-001-feat-kubernetes-workgroup-security-plan.md:1)
