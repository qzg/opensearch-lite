# Agent Guide

## Project Context

This repository is a local-only, Rust-based OpenSearch-compatible development
server. The goal is API and client compatibility with recent OpenSearch
versions at development and small-workgroup scale, not production parity for
distributed search, Lucene scoring, cluster fault tolerance, or the OpenSearch
Security plugin.

Nearby repositories are useful context but should not be modified unless the
user explicitly asks:

- `../OpenSearch` is a checkout of Apache OpenSearch for API and test parity
  reference.
- `../OpenSearch-Dashboards` is a checkout of OpenSearch Dashboards for
  application-driven API gap analysis.
- `../axon` is the target development architecture that will consume this
  server.
- `../cqlite-server` contains the related lightweight Cassandra-compatible
  service and storage-pattern inspiration.

## Implementation Guidance

- Prefer existing local patterns over new abstractions. Check nearby modules
  before adding a new layer.
- Keep compatibility behavior OpenSearch-shaped: status codes, error types,
  hints, and official-client behavior matter more than production internals.
- Durable local data should stay inspectable and agent-friendly. The project
  favors readable JSON/JSONL storage where practical.
- Route classification is a safety boundary. Known mutating, control, or
  wrong-method APIs should fail closed rather than reaching runtime agent
  fallback.
- Runtime agent fallback is privacy-sensitive. Read fallback remains the
  default; write fallback must stay explicit, route-allowlisted, request-scoped,
  and committed only through server-validated tools with tests and
  documentation.
- Authorization is based on generated route access classes, not HTTP method
  alone. Some OpenSearch read APIs use `POST`.
- Security checks should run before deterministic handlers, best-effort
  responses, body-heavy work, and runtime agent fallback.
- Do not log or echo credentials, password hashes, tokens, Authorization
  headers, private keys, or secret file contents.

## Security And Deployment Posture

- Loopback local development may run HTTP/no-auth by default.
- Non-loopback serving requires `--allow-nonlocal-listen` and, unless explicitly
  overridden for development, TLS plus `--users-file`.
- Cleartext/no-auth non-loopback serving must use
  `--allow-insecure-non-loopback` and should remain visibly exceptional.
- Kubernetes and Docker examples should use mounted TLS and auth files rather
  than secret literals in command-line arguments.
- `--validate-config` is the shell-friendly diagnostic path for mounted TLS and
  users files. It should work from local shells, `docker exec`, and
  `kubectl exec`.

## Verification

Run the narrowest useful tests while iterating, then broaden before handing off
substantial changes.

Common checks:

```sh
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
```

Useful focused checks:

```sh
cargo test --test security_surface --test tls_surface --test api_inventory
cargo test --test agent_fallback --test http_surface
cargo test --test opensearch_yaml_runner
```

Official-client smoke scripts are intentionally ignored in normal test runs.
Run them directly when changing client compatibility, TLS, auth, or connection
behavior:

```sh
scripts/run-python-client-smoke.sh
scripts/run-javascript-client-smoke.sh
scripts/run-java-client-smoke.sh
OPENSEARCH_LITE_SECURE_SMOKE=1 scripts/run-python-client-smoke.sh
OPENSEARCH_LITE_SECURE_SMOKE=1 scripts/run-javascript-client-smoke.sh
OPENSEARCH_LITE_SECURE_SMOKE=1 scripts/run-java-client-smoke.sh
```

## Documentation

- `docs/supported-apis.md` tracks implemented, best-effort, fallback, and
  unsupported OpenSearch APIs.
- `docs/compatibility.md` explains compatibility boundaries and known
  non-parity.
- `docs/security.md` and `docs/kubernetes-security.md` document TLS, auth,
  mounted Secret workflows, and agent-operable diagnostics.
- `docs/agent-fallback.md` documents runtime fallback behavior and privacy
  boundaries.
- `docs/plans/` and `docs/brainstorms/` capture feature planning context.
- `docs/solutions/` is a searchable knowledge store of documented solutions to
  past problems, organized by category with YAML frontmatter fields such as
  `module`, `tags`, and `problem_type`. It is relevant when implementing or
  debugging in documented areas because it captures prior bugs, security
  decisions, workflow patterns, and prevention rules.

## Git And Working Tree

- Preserve user changes. Do not revert unrelated dirty files.
- Keep edits scoped to the requested task.
- Do not commit, merge, push, or create a pull request unless the user asks.
