---
date: 2026-04-29
topic: opensearch-lite-local-dev-server
status: planning
---

# OpenSearch Lite Plan

## Executive Summary

Build `opensearch-lite` as a local-only, single-node OpenSearch-compatible
development server for tests, laptops, and lightweight CI. The goal is not to
reimplement OpenSearch. The goal is:

> Application code that uses normal OpenSearch clients should run against
> `opensearch-lite` locally and against a real OpenSearch cluster later without
> code changes, provided the application stays inside the documented supported
> subset.

The product shape mirrors the useful part of `cqlite-server`: compatibility
first, low memory by default, readable local persistence, real-client smoke
tests, and clear unsupported-feature errors. OpenSearch is easier at the wire
level because the core API is HTTP + JSON/NDJSON, but harder semantically
because the REST surface, mappings, search DSL, aggregations, and plugins are
large.

The MVP should start with index management, document CRUD, bulk ingestion, and
a deliberately small search subset. Full OpenSearch internals, Lucene segment
compatibility, plugins, security, Dashboards, vector search, and analytics APIs
are outside the initial identity.

---

## Problem Frame

Developers who build OpenSearch-backed applications often need a local service
that is lighter than a real OpenSearch node. Containers work, but they carry JVM
startup time, heap sizing, plugin/security defaults, disk footprint, and CI
orchestration overhead.

The desired outcome is a small local process that real OpenSearch clients can
talk to normally for common application workflows: create indexes, index
documents, bulk load fixtures, run common search queries, update/delete
documents, and reset state between tests.

The hard part is not HTTP serving. The hard part is deciding which subset is
safe to emulate without teaching applications behavior that will diverge from a
real OpenSearch cluster.

---

## Product Thesis

`opensearch-lite` should feel boring to application code:

- Connect to `http://127.0.0.1:9200`.
- Use official OpenSearch clients with normal request methods.
- Create local indexes and mappings.
- Index and bulk-index JSON documents.
- Run common search DSL queries.
- Restart and preserve local data by default.
- Switch the endpoint to a real OpenSearch cluster later without application
  code changes for the supported subset.

It should not pretend to be a distributed search engine. Unsupported OpenSearch
features should fail clearly with OpenSearch-shaped JSON errors rather than
silently approximating complex behavior.

---

## Reviewed Evidence

This plan is based on the current `cqlite-server` architecture and current
OpenSearch documentation reviewed on 2026-04-29.

**Local `cqlite-server` findings**

- `cqlite-server` proves a useful local-dev compatibility pattern: implement
  the client-facing protocol surface first, back it with server-owned local
  storage, and validate with real clients rather than synthetic-only tests.
- The current server uses a compact Rust binary, local-only defaults, explicit
  resource limits, an ephemeral mode, readable storage files, and parity smokes
  against real Cassandra.
- The OpenSearch version should preserve that philosophy while replacing
  Cassandra native protocol/CQL work with HTTP routing, JSON/NDJSON parsing,
  OpenSearch response shapes, and Query DSL evaluation.

**OpenSearch findings**

- OpenSearch's API reference describes the REST API as the main interface for
  most operations. Experimental gRPC APIs exist in the 3.x series, but REST is
  the right MVP target.
- The OpenSearch release page lists `3.6.0` as released on 2026-04-07. Use the
  latest 3.x REST shape as the planning target, while keeping the advertised
  local response version configurable.
- Official clients exist for JavaScript, Python, Ruby, Java, PHP, .NET, Go,
  Hadoop, and Rust. Initial compatibility should target Python, JavaScript, and
  Java because they cover common application stacks and exercise different
  client assumptions.
- The official API specification repository tracks OpenSearch REST route
  coverage and is a useful reference for route discovery, but the MVP should not
  try to generate the whole API surface.
- The Bulk API uses newline-delimited JSON and processes operations
  independently. Bulk helper compatibility is a first-class MVP requirement.
- Query DSL support is the largest scope risk. `match_all`, `ids`, `term`,
  `terms`, `range`, `exists`, and simple `bool` queries are a reasonable first
  subset. More advanced text analysis, scoring, aggregations, script queries,
  vector queries, and plugin query types should be staged later.

---

## Actors

- A1. Application developer: runs the local server in a dev stack or test suite.
- A2. OpenSearch client library: Python, JavaScript, Java, Go, Rust, or direct
  HTTP callers.
- A3. CI runner: needs a small predictable service that starts quickly and uses
  low memory.
- A4. Maintainer: needs a bounded compatibility matrix and tests that prevent
  accidental semantic drift.

---

## Key Flows

- F1. Local application startup
  - **Trigger:** A developer starts an application configured for OpenSearch.
  - **Actors:** A1, A2
  - **Steps:** Start `opensearch-lite`, connect a client to
    `http://127.0.0.1:9200`, run root/health/product checks, create or verify
    indexes, and continue normal application startup.
  - **Outcome:** Application code runs without a real OpenSearch node.
  - **Covered by:** R1, R2, R3, R4, R23, R24

- F2. Fixture ingestion and document use
  - **Trigger:** A dev/test workflow loads search fixtures.
  - **Actors:** A1, A2, A3
  - **Steps:** Create an index, apply mapping/settings, bulk-index documents,
    refresh, search, update, delete, and reset state.
  - **Outcome:** Common test data workflows work through official clients.
  - **Covered by:** R5, R6, R7, R8, R9, R10, R17

- F3. Search query execution
  - **Trigger:** Application code issues a supported search request.
  - **Actors:** A1, A2
  - **Steps:** Send `_search` with supported Query DSL, sort/paginate/filter
    source, and read OpenSearch-shaped hits.
  - **Outcome:** The app receives expected local results for supported query
    patterns.
  - **Covered by:** R11, R12, R13, R14, R15, R16

- F4. Compatibility failure diagnosis
  - **Trigger:** A client sends an unsupported API, query type, mapping option,
    plugin endpoint, auth/TLS expectation, or cluster operation.
  - **Actors:** A1, A2, A4
  - **Steps:** The server returns a JSON error with OpenSearch-like structure,
    a specific unsupported-feature message, and a status code close to
    OpenSearch behavior.
  - **Outcome:** Developers can decide whether to avoid the feature locally or
    use a real OpenSearch container.
  - **Covered by:** R18, R19, R20, R21, R22, R25

---

## Requirements

**HTTP and REST Compatibility**

- R1. The server must listen on a configurable HTTP address, defaulting to
  `127.0.0.1:9200`.
- R2. The first supported protocol must be HTTP/1.1 JSON/NDJSON REST. gRPC is
  explicitly deferred.
- R3. `GET /` and `HEAD /` must return product/version-shaped responses that
  official clients accept for local non-security deployments.
- R4. The server must implement basic cluster and node discovery endpoints that
  common clients call at startup: `/_cluster/health`, selected `/_cat/*`
  endpoints, and lightweight stats/info responses when a target client requires
  them.
- R5. Request parsing must handle JSON request bodies, NDJSON bulk bodies,
  query-string parameters, common content types, and bounded request sizes.
- R6. Request gzip support should be added before broad client compatibility
  because some clients expose or enable compressed request bodies. If omitted
  in the first smoke, docs must tell users to disable client compression.

**Index and Mapping Surface**

- R7. The MVP must support index lifecycle APIs used by normal applications:
  create index, delete index, get index, index exists, get mapping, put mapping,
  get settings, and put settings for a documented subset.
- R8. Mappings must preserve field names, declared field types, analyzers where
  accepted, and dynamic mapping behavior enough to drive supported query
  evaluation and metadata responses.
- R9. Unsupported mapping/settings options must be preserved when harmless for
  metadata round trips or rejected clearly when they imply semantics the local
  server cannot honor.
- R10. The server should support aliases only after direct index APIs are stable.
  Alias support is important for real applications but should not complicate
  the first document-store slice.

**Document and Bulk APIs**

- R11. The MVP must support document index/create/get/update/delete APIs for
  `/{index}/_doc/{id}`, `/{index}/_create/{id}`, generated IDs, `_source`
  retrieval, and basic version metadata.
- R12. The MVP must support `/_bulk` and `/{index}/_bulk` with `index`,
  `create`, `update`, and `delete` actions, per-item error responses, and
  NDJSON validation.
- R13. Writes must be read-your-own-write visible in the same server instance.
- R14. `refresh=true` and `refresh=wait_for` must be accepted. The local MVP may
  treat refresh as immediate visibility, but this divergence must be documented.
- R15. Document updates must support the common partial `doc` update shape.
  Scripted updates and ingest pipelines are deferred.

**Search Compatibility**

- R16. The MVP search API must support `GET/POST /_search` and
  `GET/POST /{index}/_search`.
- R17. The first Query DSL subset must include `match_all`, `ids`, `term`,
  `terms`, `range`, `exists`, simple `bool` (`must`, `filter`, `should`,
  `must_not`, `minimum_should_match`), and limited `match` behavior for text
  fields.
- R18. Search responses must include OpenSearch-shaped `took`, `timed_out`,
  `_shards`, `hits.total`, `hits.max_score`, and `hits.hits` fields, including
  `_index`, `_id`, `_score`, and `_source`.
- R19. The MVP must support `from`, `size`, source filtering, simple field sort,
  and basic `track_total_hits` behavior for bounded local result sets.
- R20. Unsupported search features such as aggregations, highlighting, script
  fields, nested queries, percolator, suggesters, scroll, PIT, vector search,
  and plugin queries must return explicit unsupported errors until implemented.

**Storage and Memory**

- R21. Data must persist under `--data-dir` by default. An explicit
  `--ephemeral` mode may skip durable files for tests.
- R22. Storage must be readable by humans and coding agents. The first layout
  should use JSON metadata and JSONL document/mutation files, not OpenSearch
  segment compatibility.
- R23. The server must append durable mutation records before acknowledging
  writes in durable mode.
- R24. Restart recovery must rebuild index catalog, mappings/settings, document
  state, tombstones, and versions from local files.
- R25. The server must bound request body size, bulk action count, result page
  size, index count, document count, in-memory bytes, and concurrent
  connections.

**Compatibility and Operations**

- R26. Defaults must be conservative: localhost binding, no auth, no TLS, no
  plugins, low memory, limited bulk size, limited result size, and clear logs.
- R27. The server must provide OpenSearch-shaped JSON error bodies with stable
  status codes for unsupported APIs, invalid request bodies, unknown indexes,
  version conflicts, and request-size violations.
- R28. Compatibility must be verified with real clients, starting with Python
  `opensearch-py`, JavaScript `@opensearch-project/opensearch`, and the
  OpenSearch Java client.
- R29. The same smoke flow must run against `opensearch-lite` and a real
  OpenSearch container, and accepted divergences must be documented.
- R30. Documentation must include a compatibility matrix, supported API subset,
  unsupported features, client configuration examples, and migration guidance
  to real OpenSearch.

---

## Acceptance Examples

- AE1. **Covers R1, R2, R3, R4.** Given
  `opensearch-lite --listen 127.0.0.1:9200 --data-dir ./data`, when a Python
  `opensearch-py` client connects and calls `info()` plus
  `cluster.health()`, both calls succeed without custom transport code.

- AE2. **Covers R7, R8, R11, R13.** Given an index `books` with a simple mapping,
  when a client indexes document `1` and then gets it by ID, the response
  includes `_source`, `_id`, `_index`, `found: true`, and expected version
  metadata.

- AE3. **Covers R12.** Given a bulk request containing successful `index`,
  `update`, and `delete` actions plus one invalid action, the response reports
  per-item statuses and continues processing after the invalid item.

- AE4. **Covers R16, R17, R18, R19.** Given several documents in `books`, when a
  client searches with a `bool` query containing `term` and `range` filters,
  the response includes only matching hits, honors `from`/`size`, and returns an
  OpenSearch-shaped `hits.total`.

- AE5. **Covers R20, R27.** Given a client sends an aggregation or vector query
  before those are supported, the server rejects the request with a JSON error
  that names the unsupported feature instead of returning an approximate result.

- AE6. **Covers R21, R23, R24.** Given a document was indexed in durable mode
  and the server stops cleanly, when it restarts with the same `--data-dir`, a
  supported `_search` and `_doc/{id}` request both return the document.

---

## Scope Boundaries

### First MVP

- HTTP/JSON REST only, no gRPC.
- Single-node local behavior only.
- Direct index APIs before aliases.
- JSON documents and schema-lite mappings.
- Document CRUD, bulk, and small Query DSL subset.
- Readable local JSON/JSONL storage.
- Official-client compatibility tests for Python, JavaScript, and Java.

### Deferred For Later

- Aliases and index templates.
- Multi-search, multi-get, delete-by-query, update-by-query, reindex.
- Scroll, point-in-time, search-after, and deep pagination.
- Aggregations.
- Highlighting and suggesters.
- Full analyzer/tokenizer parity.
- Nested, parent/child, join, percolator, and geo queries.
- SQL, PPL, Dashboards, alerting, observability, ML, security analytics, and
  other plugin APIs.
- Vector search and hybrid search.
- TLS, authentication, roles, tenancy, and AWS SigV4 compatibility.
- OpenSearch Serverless-specific behavior.

### Outside This Product's Identity

- Production search engine replacement.
- Lucene/OpenSearch segment compatibility.
- Distributed cluster behavior, shard allocation, replicas, recovery, and node
  coordination.
- Matching OpenSearch performance, scoring, or analyzer internals exactly.
- Running OpenSearch plugins.
- Forking OpenSearch to make it smaller.

---

## Architecture Recommendation

Use a layered Rust architecture:

```text
Official OpenSearch clients / HTTP callers
        |
        v
HTTP listener and request router
        |
        v
Request normalization and OpenSearch response/error shaping
        |
        +--------------------+--------------------+
        |                    |                    |
        v                    v                    v
Cluster/index APIs      Document APIs        Search APIs
        |                    |                    |
        v                    v                    v
Catalog/mapping store   Document store       Query DSL evaluator
        |                    |                    |
        +--------------------+--------------------+
                             |
                             v
              JSONL mutation log + compacted snapshots
```

### Component Responsibilities

**HTTP listener and router**

- Accept HTTP/1.1 requests on a configurable local address.
- Route method/path combinations to typed handlers.
- Enforce body size and connection limits before expensive parsing.
- Preserve OpenSearch-ish method/path behavior, including `HEAD` status-only
  responses.

**Request normalization**

- Parse JSON and NDJSON bodies.
- Normalize query-string parameters such as `refresh`, `routing`, `pretty`,
  `_source`, `from`, `size`, and `track_total_hits`.
- Decide which unknown parameters are ignored for compatibility and which are
  rejected because they imply unsupported semantics.

**Response and error shaping**

- Produce response bodies that official clients accept.
- Keep status codes and JSON error structure close to OpenSearch.
- Prefer explicit unsupported-feature errors over partial emulation.

**Catalog and mapping store**

- Persist index names, UUIDs, settings, mappings, aliases once supported, and
  compatibility metadata.
- Generate `GET /{index}`, mapping, and settings responses from local state.
- Preserve unsupported but harmless mapping/settings fields for round-trip
  metadata where possible.

**Document store**

- Store documents by index and ID.
- Maintain document version, sequence number, primary term placeholder, source,
  tombstone state, and update timestamp.
- Append a mutation before acknowledging writes in durable mode.
- Compact mutation logs into index-shaped snapshots.

**Query DSL evaluator**

- Evaluate the supported Query DSL subset against bounded local document sets.
- Start with deterministic filtering and simple scoring.
- Keep scoring intentionally modest and documented; exact OpenSearch/Lucene
  scoring parity is not an MVP requirement.

---

## Project Shape

Proposed files under `opensearch-lite/`:

```text
opensearch-lite/
  PLAN.md
  Cargo.toml
  src/
    main.rs
    lib.rs
    config.rs
    server.rs
    http/
      mod.rs
      router.rs
      request.rs
      response.rs
      errors.rs
    api/
      cluster.rs
      cat.rs
      indices.rs
      documents.rs
      bulk.rs
      search.rs
    catalog/
      mod.rs
      mapping.rs
      settings.rs
      persist.rs
    storage/
      mod.rs
      mutation_log.rs
      snapshots.rs
      document_store.rs
    query/
      mod.rs
      dsl.rs
      evaluator.rs
      source_filter.rs
      sort.rs
  tests/
    http_surface.rs
    index_surface.rs
    document_surface.rs
    bulk_surface.rs
    search_surface.rs
    python_client_smoke.rs
    javascript_client_smoke.rs
    java_client_smoke.rs
  docker/
    docker-compose.yml
    client-smoke/
  docs/
    compatibility.md
    supported-apis.md
    driver-examples.md
```

This is a planning target, not a requirement to create all files upfront.

---

## Storage Plan

Use an AI-readable local store rather than OpenSearch/Lucene-compatible segment
files.

Initial durable layout:

```text
data/
  cluster.json
  indices/
    <index>/
      manifest.json
      mapping.json
      settings.json
      mutations.jsonl
      documents.jsonl
      tombstones.jsonl
```

Recommended semantics:

- `cluster.json` stores cluster UUID, advertised version, and local node ID.
- `manifest.json` stores index UUID, creation time, document counters, and file
  generation metadata.
- `mapping.json` and `settings.json` preserve accepted request bodies after
  validation/normalization.
- `mutations.jsonl` records acknowledged write operations before they affect
  durable snapshots.
- `documents.jsonl` stores compacted current documents with `_id`, `_source`,
  version, sequence number, primary term placeholder, and update timestamp.
- `tombstones.jsonl` stores deletes only as long as needed for version/conflict
  semantics.
- Startup replays snapshots then mutations into an in-memory document map.
- `--ephemeral` skips local file writes entirely.

Future maturation can split mutations into numbered segments, add manifests for
safe compaction, and add optional inverted indexes or Tantivy-backed search once
the brute-force evaluator's limits are reached.

---

## Supported API Scope

### First Externally Useful MVP

Cluster/info:

- `GET /`
- `HEAD /`
- `GET /_cluster/health`
- `GET /_cat/health`
- `GET /_cat/indices`

Index APIs:

- `PUT /{index}`
- `DELETE /{index}`
- `GET /{index}`
- `HEAD /{index}`
- `GET /{index}/_mapping`
- `PUT /{index}/_mapping`
- `GET /{index}/_settings`
- `PUT /{index}/_settings`

Document APIs:

- `PUT /{index}/_doc/{id}`
- `POST /{index}/_doc`
- `PUT /{index}/_create/{id}`
- `POST /{index}/_create/{id}`
- `GET /{index}/_doc/{id}`
- `HEAD /{index}/_doc/{id}`
- `POST /{index}/_update/{id}`
- `DELETE /{index}/_doc/{id}`
- `POST /_bulk`
- `PUT /_bulk`
- `POST /{index}/_bulk`
- `PUT /{index}/_bulk`

Search APIs:

- `GET /_search`
- `POST /_search`
- `GET /{index}/_search`
- `POST /{index}/_search`

Query DSL:

- `match_all`
- `ids`
- `term`
- `terms`
- `range`
- `exists`
- `bool` with `must`, `filter`, `should`, `must_not`, and basic
  `minimum_should_match`
- Limited `match` for text fields, likely lowercase token containment at first

Response features:

- `_source` includes/excludes
- `from`
- `size`
- simple field sort
- `track_total_hits`
- basic `_score`

### Beta Scope

- Aliases.
- Multi-get.
- Multi-search.
- Delete by query for supported Query DSL.
- Update by query for supported Query DSL.
- Index templates.
- Better text analysis with documented analyzer subset.
- Docker image and Compose example.
- Optional request/response gzip.

---

## Driver Compatibility Targets

Initial targets, in order:

1. Python `opensearch-py`
2. JavaScript `@opensearch-project/opensearch`
3. OpenSearch Java client
4. Go client, if relevant to target applications
5. Rust client, if relevant later

Baseline local client configuration:

```text
host = 127.0.0.1
port = 9200
scheme = http
auth = disabled
tls = disabled
compression = disabled until gzip support lands
```

The first implementation task should capture the exact requests each client
sends for connect/info/health/index/document/search flows. Those traces become
the compatibility contract for the MVP.

---

## Delivery Milestones

### M0: Project Scaffold and HTTP Fixtures

Deliver:

- Rust crate scaffold.
- CLI config for `--listen`, `--data-dir`, `--ephemeral`,
  `--memory-limit`, `--max-body-size`, `--bulk-action-limit`,
  and `--result-size-limit`.
- HTTP listener and route dispatch.
- OpenSearch-shaped root response and error body helpers.
- Unit tests for route matching, JSON parsing, NDJSON parsing, and error
  shaping.

Exit criteria:

- `GET /`, `HEAD /`, and an unknown route return expected status/body shapes.
- Oversized request bodies fail before unbounded allocation.
- The server defaults to `127.0.0.1:9200`.

### M1: Cluster and Index Catalog

Deliver:

- `/_cluster/health`.
- `/_cat/health`.
- `/_cat/indices`.
- Create/delete/get/head index.
- Durable catalog with mapping/settings snapshots.
- Basic OpenSearch index metadata response shape.

Exit criteria:

- A real Python client can connect, call info/health, create an index, check
  existence, get index metadata, and delete the index.

### M2: Document CRUD and Durable Store

Deliver:

- Document index/create/get/head/update/delete.
- Generated IDs.
- Basic version and sequence metadata.
- JSONL mutation log and startup replay.
- Compaction to `documents.jsonl`.
- Ephemeral mode parity for CRUD.

Exit criteria:

- Python and JavaScript clients can create an index, index a document, get it,
  update it, delete it, and observe not-found responses after deletion.
- Durable restart returns previously indexed documents.

### M3: Bulk API and Client Parity Harness

Deliver:

- `/_bulk` and `/{index}/_bulk`.
- Per-item success/error responses.
- Bulk limits for body size, action count, and generated ID count.
- Docker/Compose harness that runs the same smoke flow against
  `opensearch-lite` and a real OpenSearch 3.x container.
- Java client smoke.

Exit criteria:

- Python, JavaScript, and Java client smokes pass against `opensearch-lite`.
- The same basic bulk/index/get/search flow passes against a real OpenSearch
  container, with documented accepted divergences.

### M4: Search MVP

Deliver:

- `_search` endpoints.
- Query DSL evaluator for the first subset.
- Source filtering.
- `from`/`size`.
- Simple sort.
- `track_total_hits`.
- OpenSearch-shaped hits response.
- Explicit unsupported errors for unimplemented query and aggregation features.

Exit criteria:

- Real clients can run supported searches and receive expected hit sets.
- Unsupported aggregations/vector/script/nested requests fail clearly.
- Result size limits prevent unbounded materialization.

### M5: Mapping and Analysis Hardening

Deliver:

- Better field type validation.
- Dynamic mapping behavior for common scalar fields.
- Basic keyword/text differences.
- Simple lowercase text analysis for `match`.
- Date/numeric range handling that follows mapping type.
- Mapping round-trip compatibility improvements discovered by real clients.

Exit criteria:

- Search results for common text/keyword/numeric/date fields match the
  documented local semantics.
- Mapping errors are clear enough to steer developers toward supported shapes.

### M6: Dev-Stack Beta

Deliver:

- Container image.
- Compose example.
- Compatibility matrix.
- Supported API docs.
- Client examples for Python, JavaScript, and Java.
- App-call inventory guide for projects deciding whether `opensearch-lite`
  fits their workflow.

Exit criteria:

- A sample app can switch between `opensearch-lite` and OpenSearch by changing
  endpoint configuration only.
- CI can use `opensearch-lite` for supported local search tests.

---

## Testing Plan

### HTTP and API Surface Tests

- Method/path route matching.
- `HEAD` status-only responses.
- Query-string parsing.
- JSON content-type handling.
- NDJSON bulk parsing with trailing newline and malformed-line cases.
- Request body limits.
- Unknown route errors.
- OpenSearch-shaped error bodies.

### Catalog Tests

- Create index with and without mapping/settings.
- Duplicate create returns resource-already-exists-style error.
- Delete index removes metadata and documents.
- Get/head index status behavior.
- Mapping/settings round-trip for accepted fields.
- Unsupported settings either preserve or reject according to compatibility
  rules.

### Document Tests

- Index with explicit ID.
- Create with explicit ID rejects existing document.
- Auto-generated ID indexes successfully.
- Get/head existing and missing documents.
- Partial update merges fields.
- Delete tombstones document.
- Version/sequence metadata increments predictably.
- Durable restart preserves writes and deletes.

### Bulk Tests

- Mixed `index`, `create`, `update`, and `delete` actions.
- Per-item failure does not stop subsequent valid actions.
- Missing `_index` fails unless path index is provided.
- Malformed NDJSON returns a structured error.
- Bulk action limits fail predictably.

### Search Tests

- `match_all` returns all live docs.
- `ids` returns selected docs.
- `term` and `terms` match keyword/numeric fields.
- `range` handles numeric and date-like fields.
- `exists` handles present and missing fields.
- `bool` combines `must`, `filter`, `should`, and `must_not`.
- `from`/`size` bounds results.
- Source filtering includes/excludes expected fields.
- Sort orders by a simple scalar field.
- Unsupported query types and aggregations return explicit errors.

### Client Compatibility Tests

Run the same high-level flow through real clients:

```text
info
cluster health
create index with mapping
index document
bulk index/update/delete
get document
search supported query
delete document
delete index
```

Run every smoke against:

1. `opensearch-lite`
2. A real OpenSearch 3.x container

Document any accepted divergence in `docs/compatibility.md`.

---

## Key Trade-Offs

- Prefer real-client compatibility over broad API coverage.
- Prefer clear unsupported errors over approximate behavior.
- Prefer readable local JSON/JSONL storage over OpenSearch segment
  compatibility.
- Prefer deterministic local semantics over distributed-cluster simulation.
- Prefer brute-force bounded search first, then add an index only when tests
  prove brute force is insufficient.
- Prefer a small documented Query DSL subset over partial support for many query
  types.
- Prefer localhost/no-auth/no-TLS defaults for local dev, with security features
  deferred.

---

## Risk Register

| Risk | Severity | Evidence | Mitigation |
|---|---:|---|---|
| Query DSL scope expands without bound | High | OpenSearch has many query types and plugins | Start from an app-call inventory and document unsupported query errors |
| Client startup checks require hidden endpoints | Medium | Official clients may call info, health, product/version, or compatibility headers | Capture exact client request traces in M1 and add only required endpoints |
| Bulk helpers rely on nuanced response details | High | Bulk APIs return per-item statuses and continue after item failures | Implement bulk before advanced search; parity-smoke against real OpenSearch |
| Mapping/analyzer behavior diverges from real OpenSearch | High | Text analysis and field types affect search results | Document local semantics; defer advanced analyzers; add parity cases for common fields |
| Memory grows during search result materialization | High | Brute-force search can collect too many docs | Enforce result limits, max docs, max body, and source-size budgets |
| Developers mistake local semantics for production search | Medium | A lightweight emulator can hide performance/scoring differences | Strong compatibility matrix and unsupported-feature errors |
| Security/TLS assumptions block client connection | Medium | Default OpenSearch examples often use HTTPS with demo auth | Document local HTTP/no-auth config and add TLS/auth only if target apps require it |
| Current OpenSearch version shifts | Low | OpenSearch releases continue through 2026 | Pin tested container versions in smokes and keep advertised version configurable |

---

## Outstanding Questions

### Resolve Before Implementation

- Which target application or stack should define the first API inventory?
- Which official clients are mandatory for the first beta: Python, JavaScript,
  Java, or another language?
- Does the target app require aliases or index templates before search MVP?
- Does the target app enable request compression by default?
- Should the advertised root version be latest OpenSearch 3.x, OpenSearch
  2.x-compatible, or configurable per test suite?

### Deferred To Implementation Planning

- Exact Rust HTTP stack choice after scaffold evaluation.
- Exact JSONL compaction thresholds.
- Whether to introduce Tantivy or another indexing library after brute-force
  search reaches limits.
- Exact error type names for each unsupported feature after comparing real
  OpenSearch responses.
- Whether to preserve unknown mapping/settings fields by default or require an
  allowlist per setting group.

---

## Recommended Next Step

Start with an API inventory before coding:

1. Run the target application or representative smoke flow against a real
   OpenSearch 3.x container.
2. Capture method, path, query parameters, content type, and request body shape
   for each OpenSearch call.
3. Classify calls into MVP, beta, deferred, or unsupported.
4. Convert the MVP call list into route fixtures and client smoke tests.
5. Scaffold M0 only after that inventory confirms the first compatibility
   target.

This keeps `opensearch-lite` bounded by actual application needs instead of
drifting toward a partial clone of the full OpenSearch REST API.

---

## Sources and References

- `cqlite-server` plan and current implementation: sibling project
  `cqlite-server/`
- OpenSearch release schedule and version history:
  <https://opensearch.org/releases/>
- OpenSearch API reference:
  <https://docs.opensearch.org/latest/api-reference/>
- OpenSearch core index APIs:
  <https://docs.opensearch.org/latest/api-reference/index-apis/core-index-apis/>
- OpenSearch document APIs:
  <https://docs.opensearch.org/latest/api-reference/document-apis/index/>
- OpenSearch Bulk API:
  <https://docs.opensearch.org/latest/api-reference/document-apis/bulk/>
- OpenSearch Search API:
  <https://docs.opensearch.org/latest/api-reference/search-apis/search/>
- OpenSearch Query DSL bool query:
  <https://docs.opensearch.org/latest/query-dsl/compound/bool/>
- OpenSearch language clients:
  <https://docs.opensearch.org/latest/clients/>
- OpenSearch API specification repository:
  <https://github.com/opensearch-project/opensearch-api-specification>
