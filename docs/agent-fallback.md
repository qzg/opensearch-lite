# Agent Fallback

Runtime agent fallback is a configured compatibility mechanism for unsupported
or agent-assisted routes. Read fallback is enabled by `--agent-endpoint`.
Write fallback is still disabled unless `--agent-enable-write-fallback` and
`--agent-write-allowlist` explicitly allow the API name. Fallback runs after
authentication and authorization. It cannot answer unauthenticated or
unauthorized requests, and it is denied for security/control namespaces such as
`_plugins/_security`, `_opendistro/_security`, `_security`, snapshots, and
task-control APIs. Implemented snapshot repository management routes are
handled deterministically after admin authorization in durable mode; snapshot
APIs fail closed under `--ephemeral`, and unsupported snapshot subpaths fail
closed. Snapshot restore, clone, and status routes are recognized admin APIs
that return unsupported responses without invoking fallback. PIT lifecycle
routes are deterministic read-class runtime
operations, and `_search` with `pit.id` uses the retained frozen runtime view.
`search_after` cursor paging is deterministic for `_search`, including
non-unique sort values; `_msearch` requests with PIT or `search_after` still
fail closed instead of reaching
fallback. The exact Dashboards account probe
`GET /_plugins/_security/api/account` is a deterministic mocked route and does
not use fallback. The exact direct-query data-source probe
`GET /_plugins/_query/_datasources` is also a deterministic mocked route and
does not use fallback.

First-tranche Dashboards fixture APIs are deterministic and do not use fallback:
`indices.exists`, `field_caps`, `cat.plugins`, `cat.templates`,
`cluster.stats`, `indices.resolve_index`, `query.datasources`, snapshot
repository create/get/delete, snapshot create/get/delete, PIT lifecycle
create/list/delete, legacy template delete, alias updates, Discover-style
search, and the documented visualization aggregation subset.

## Configuration

```sh
mainstack-search \
  --agent-endpoint https://example.test/v1/chat/completions \
  --agent-model model-name \
  --agent-token-env OPENAI_API_KEY
```

The endpoint must be OpenAI chat-completions compatible. Local endpoints may use
plain HTTP on loopback addresses. Non-loopback HTTP endpoints are rejected unless
`--agent-allow-insecure-endpoint` is explicitly set.

Bearer tokens must be loaded from an environment variable or a secret file. The
server must not log token material in startup logs, request logs, debug output,
or errors.

Write fallback requires an additional local-development opt-in:

```sh
mainstack-search \
  --agent-endpoint https://example.test/v1/chat/completions \
  --agent-model model-name \
  --agent-token-env OPENAI_API_KEY \
  --agent-enable-write-fallback \
  --agent-write-allowlist indices.put_template
```

The ignored local `.env` can hold a concrete OpenAI-compatible backend
selection. For the current recommended cheap hosted model:

```sh
MAINSTACK_SEARCH_AGENT_ENDPOINT=https://openrouter.ai/api/v1/chat/completions
MAINSTACK_SEARCH_AGENT_MODEL=deepseek/deepseek-v4-flash
MAINSTACK_SEARCH_AGENT_TOKEN_ENV=OPENROUTER_API_KEY
```

Run the server from that configuration with:

```sh
set -a
. ./.env
set +a
cargo run -- \
  --agent-endpoint "$MAINSTACK_SEARCH_AGENT_ENDPOINT" \
  --agent-model "$MAINSTACK_SEARCH_AGENT_MODEL" \
  --agent-token-env "$MAINSTACK_SEARCH_AGENT_TOKEN_ENV"
```

The live backend tests are intentionally ignored because they use network and
paid model calls. They include smoke checks plus the benchmark fixture
regression suite, which grades each fixture and fails with per-check reasons
when the configured runtime model falls below the fixture threshold. To
exercise the real fallback backend:

```sh
set -a
. ./.env
set +a
MAINSTACK_SEARCH_LIVE_AGENT_TEST=1 \
cargo test --test live_agent_backend -- --ignored --test-threads=1
```

The model can own the OpenSearch-shaped response, but it cannot write files
directly. Durable side effects must appear as a `commit_mutations` tool call in
the wrapper. The server validates the tool scope, authorization, memory limits,
and storage mutation before committing. A successful write response that claims
side effects without a successful commit is rejected.

## Data Exposure

Once configured, the endpoint is trusted. Fallback context may include:

- the incoming request method/path/query/body
- route tier and compatibility metadata
- index templates, aliases, mappings, and settings
- bounded raw local documents from the target index scope

For known OpenSearch read APIs, the server scopes fallback context to indices
named in the request path or query parameters. Unknown fallback routes receive
metadata only even if the caller supplies `index` or `indices` query parameters.
If a request does not identify a validated target index, fallback receives
metadata only plus omission counts. Cloud-hosted endpoints may still receive
local indexed data for targeted requests. Do not enable fallback for private
data unless that trust boundary is acceptable.

Authorization headers and secret-like query or body fields are redacted before
fallback context is constructed. If a coding agent receives an authn/authz
failure, it should adjust credentials or choose a permitted read API rather
than expecting fallback to complete the request.

## Response Contract

The model must return only a JSON wrapper:

```json
{
  "status": 200,
  "headers": {},
  "body": {},
  "confidence": 90,
  "failure_reason": null,
  "read_only": true,
  "tool_calls": []
}
```

`body` is the OpenSearch-shaped JSON response returned to the caller after
validation. The server rejects malformed wrappers, low confidence, oversized
responses, unauthorized write intent, successful write claims without a commit,
and invalid status values.

Raw documents are serialized as quoted data with stable delimiters and are
treated as untrusted. Document text must not override system or developer
instructions.

## Durable Files And Agents

Coding agents with local filesystem access can inspect durable development data
directly under `--data-dir`. `mutations.jsonl` is an append-first JSONL log with
transaction `begin` and `commit` records; each begin record includes readable
mutation kinds such as `create_index`, `index_document`, `update_document`, and
`delete_document`. `snapshot.json`, when present, is materialized JSON state.
`snapshot.meta.json` is a small readable metadata file with generation,
estimated stored bytes, index count, document count, registry object count, and
the mutation-log high-water mark. Snapshots flush after 1000 dirty writes or 10
dirty minutes on the next write, unless configured otherwise.

This direct inspection path is for local development and synthetic/debug data.
Agents should avoid printing arbitrary document bodies, auth material, users
files, private keys, or token-like values while summarizing durable state.
