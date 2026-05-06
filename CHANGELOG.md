# Changelog

## 2026-05-05 — Renamed from `opensearch-lite` to `mainstack-search`

This repo was renamed from `opensearch-lite` to `mainstack-search` to align
with the MAIN Stack umbrella project. The previous name reflected an earlier
identity (Axon-era).

### Breaking changes

- **Wire-visible / persisted identity strings.** The following all changed
  prefix from `opensearch-lite` to `mainstack-search`. Existing `data/`
  directories will need to be wiped or accepted as residue with stale names.
  Clients holding stale PIT-IDs must re-acquire them.
  - **PIT-IDs** (`opensearch-lite-pit:…` → `mainstack-search-pit:…`).
    Clients persist these across requests; post-rename the server will not
    recognize old IDs.
  - **`cluster_name`** (`"opensearch-lite"` → `"mainstack-search"`).
  - **`cluster_uuid`** (`opensearch-lite-{:x}` → `mainstack-search-{:x}`).
  - **`node` name** (`"opensearch-lite"` → `"mainstack-search"`).
  - **Index `uuid`** pattern (`opensearch-lite-{name}` →
    `mainstack-search-{name}`). Persisted in index metadata.
  - **Snapshot identifiers** (`opensearch-lite-local`,
    `opensearch-lite-{now}-{:016x}` → `mainstack-search-…`). Persisted on
    disk.
  - **On-disk lock file** (`.opensearch-lite.lock` → `.mainstack-search.lock`).
    Stale lock from old binary is harmless residue.
  - **Security salt** (`b"opensearch-lite"` → `b"mainstack-search"`).
    Auth tokens or derived keys generated pre-rename will not be accepted
    post-rename.

### Identity changes (silent unless you check)

- Cargo crate / binary: `opensearch-lite` → `mainstack-search`.
- Rust lib path: `opensearch_lite` → `mainstack_search`.
- Env var prefix: `OPENSEARCH_LITE_*` → `MAINSTACK_SEARCH_*` (22 distinct
  variables across server config, agent fallback, smoke scripts, and
  benchmarks). Old names are silently ignored — code falls back to defaults
  rather than erroring (the consumers use `unwrap_or_else(default)` /
  `.ok()`), so misconfiguration will not surface until behavior diverges.
  Update shells, CI, `.envrc`, direnv configs, smoke/bench scripts.
- Java package: `local.opensearchlite` → `local.mainstacksearch`.
- Docker image / Dockerfile: `opensearch-lite.Dockerfile` →
  `mainstack-search.Dockerfile`; image tag `opensearch-lite:latest` →
  `mainstack-search:latest`; `opensearch-lite-java-smoke` →
  `mainstack-search-java-smoke`.
- Smoke test passwords: `opensearch-lite-smoke-password` →
  `mainstack-search-smoke-password`; `OpenSearchLite1!` →
  `MainstackSearch1!`. Update any pinned test fixtures.
- Mounted CA path: `/run/opensearch-lite/ca.pem` →
  `/run/mainstack-search/ca.pem`.
- Cross-repo references: `../axon` → `../Mainstack`; `../cqlite-server` →
  `../mainstack-cql`.
- Docs naming: index parity smoke now uses `mainstack-parity` (was
  `axon-parity`).
- Git remote: `git@github.com:qzg/opensearch-lite.git` →
  `git@github.com:qzg/mainstack-search.git`.

### What is preserved (upstream references)

The following are upstream protocol/product references and are NOT renamed:

- `OpenSearch` (Apache OpenSearch — the upstream product this server is
  compatible with).
- `OpenSearch Dashboards` (the upstream UI).
- `vendor/opensearch-rest-api-spec/` (vendored upstream spec).
- `opensearch-py`, `cqlsh-rs`, and similar client/sibling tool references.

### Agent-host state

The Codex per-project config (`~/.codex/config.toml`
`[projects."…"]` headers) and Claude session-history slug directories
(`~/.claude/projects/-home-…-opensearch-lite/`) were moved in lockstep
with this rename to preserve session continuity. Append-only history
(`~/.codex/sessions/`, `~/.claude/history.jsonl`,
`~/.codex/history.jsonl`) retains old project names; this is
expected residue and not a regression.
