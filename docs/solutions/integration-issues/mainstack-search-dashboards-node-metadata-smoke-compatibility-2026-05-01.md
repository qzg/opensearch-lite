---
title: mainstack-search Dashboards Node Metadata Smoke Compatibility
date: 2026-05-01
category: integration-issues
module: mainstack-search dashboards metadata api
problem_type: integration_issue
component: tooling
symptoms:
  - "OpenSearch Dashboards Docker smoke reported that it could not retrieve version information from OpenSearch nodes"
  - "GET /_nodes with Dashboards filter_path returned an empty nodes map"
  - "The broad /_nodes best-effort handler could return nodes.info shape for distinct nodes.stats routes"
  - "Node metadata could report a configured publish address but a hard-coded loopback ip"
root_cause: wrong_api
resolution_type: code_fix
severity: medium
related_components:
  - testing_framework
  - documentation
tags:
  - mainstack-search
  - opensearch-dashboards
  - api-compatibility
  - nodes-info
  - nodes-stats
  - filter-path
  - docker-smoke
---

# mainstack-search Dashboards Node Metadata Smoke Compatibility

## Problem

The first Docker-hosted OpenSearch Dashboards smoke exposed a node metadata
compatibility gap in mainstack-search. Dashboards booted far enough to ask the
server for node versions, but the best-effort `/_nodes` response returned no
nodes, so Dashboards could not confirm the connected OpenSearch version.

## Symptoms

- OpenSearch Dashboards logged that it was unable to retrieve version
  information from OpenSearch nodes.
- The observed request was
  `GET /_nodes?filter_path=nodes.*.version,nodes.*.http.publish_address,nodes.*.ip`.
- mainstack-search answered the route as best-effort metadata, but the body was
  only an empty `nodes` object.
- The initial fix risked over-broadening the response because every `/_nodes*`
  path could receive a `nodes.info` body, including `/_nodes/stats`.

## What Didn't Work

- Treating `nodes.info` as generic empty metadata was too weak for Dashboards.
  Dashboards needs at least one node entry with a version to proceed past its
  compatibility check.
- Returning a non-empty body from a broad `path.starts_with("/_nodes")` branch
  solved the boot blocker but blurred API contracts. `nodes.info` and
  `nodes.stats` are distinct OpenSearch APIs and should not share one response
  shape accidentally.
- Hard-coding `"ip": "127.0.0.1"` while deriving `http.publish_address` from
  `state.config.listen` made non-default loopback, Docker, IPv6, and workgroup
  listener configurations internally inconsistent.
- Documenting the successful live smoke only in the gap analysis left formal
  compatibility docs stale; future agents could still think live Dashboards
  startup had not been attempted.

## Solution

mainstack-search now gives `nodes.info` a real single-node best-effort response,
but keeps route classification deliberate.

The handler dispatches by classified API name instead of a broad path prefix:

```rust
match api_name {
    "nodes.info" => {
        let node_ip = state.config.listen.ip().to_string();
        best_effort::nodes_info(
            api_name,
            &state.config.advertised_version,
            &node_ip,
            &state.config.listen.to_string(),
            request.query_value("filter_path"),
        )
    }
    "nodes.stats" => handle_nodes_stats(&state, &request, api_name),
    _ => { /* existing best-effort handlers */ }
}
```

`nodes.info` now returns configured metadata for the local node and applies the
Dashboards `filter_path` so the response only includes requested fields when a
client asks for a filtered shape:

```json
{
  "nodes": {
    "mainstack-search-local-node": {
      "version": "3.6.0",
      "ip": "127.0.0.1",
      "http": {
        "publish_address": "127.0.0.1:9200"
      }
    }
  }
}
```

Route classification now separates valid `nodes.info` and `nodes.stats` shapes.
Malformed extra-segment forms are classified as unsupported for the known node
API family instead of falling through to generic GET fallback:

```rust
if nodes_stats_path(&segments) {
    return get_only(method, "nodes.stats", Tier::BestEffort);
}
if nodes_stats_family(&segments) {
    return unsupported_method("nodes.stats", method);
}
if nodes_info_path(&segments) {
    return get_only(method, "nodes.info", Tier::BestEffort);
}
if segments.first() == Some(&"_nodes") {
    return unsupported_method("nodes.info", method);
}
```

The fixture coverage now checks:

- Dashboards' exact `/_nodes?filter_path=...` request shape.
- `x-mainstack-search-api` and `x-mainstack-search-tier` compatibility headers.
- Non-default configured version and listener metadata.
- Distinct `nodes.stats` best-effort response shape.
- Filtered `nodes.stats` responses.
- Extra-segment `nodes.info` and `nodes.stats` paths failing closed.

Compatibility docs were also updated to record the Docker smoke result: a
Docker-hosted OpenSearch Dashboards 3.6.0 process reached green status with
security disabled and passed synthetic data-view field discovery, saved-object
index-pattern creation, and Discover-style `_msearch` route probes.

## Why This Works

The root cause was returning the wrong API contract for an application-driven
metadata path. OpenSearch Dashboards was not asking for full cluster behavior;
it needed a precise subset of `nodes.info` fields to confirm the server version.
Providing that subset deterministically keeps startup compatibility local and
does not involve runtime agent fallback.

The route split matters as much as the body shape. Best-effort responses are
still contracts: if a client calls a known but malformed node route, the server
should fail closed for that known API family instead of silently turning it into
another successful response or generic fallback.

## Prevention

- Treat live application smoke traces as API contracts. Capture the exact
  method, path, query parameters, and response shape the application consumes.
- Avoid broad prefix handlers for OpenSearch API families where sibling routes
  have different contracts. Route on the classified API name when possible.
- For best-effort metadata that reflects server identity, derive related fields
  from the same configured source.
- Add tests for both valid and malformed route shapes whenever broad
  compatibility handlers are introduced.
- Keep `docs/compatibility.md`, `docs/supported-apis.md`, and the
  Dashboards gap analysis aligned when a live smoke changes the compatibility
  claim boundary.

## Related Issues

- `docs/opensearch-dashboards-gap-analysis.md` records the successful Docker
  smoke and the observed OpenSearch API traffic.
- `docs/compatibility.md` states the current narrow Dashboards compatibility
  claim and the remaining browser-driven workflow boundary.
- `docs/supported-apis.md` tracks `nodes.info` and `nodes.stats` as
  best-effort metadata APIs.
