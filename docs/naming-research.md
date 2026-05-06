# Naming Research Notes

Date: 2026-04-29

This note captures the product and component naming discussion so future work can
continue in a separate thread without reopening the full conversation history.
It is not legal advice; names intended for external IBM or client-facing use
should go through IBM brand/legal clearance.

## Product Context

The production product is intended to drive consumption of data products under
the `watsonx.data` portfolio umbrella. The full production deployment is expected
to use production-grade Kafka, Cassandra, OpenSearch, watsonx.data lakehouse
capabilities with Spark and Presto, and a full OpenShift cluster, likely in a
multi-tenant setup.

The lightweight services currently being built are intended for local
development and small workgroup-sized deployments. They should expose
production-compatible interfaces so applications can move to production
infrastructure without code changes.

Current local service working names:

- `mainstack-search`: Rust OpenSearch-compatible local search server.
- `mainstack-cql`: Rust Cassandra/CQL-compatible local server.
- `axon`: former architecture/project name for the larger runtime.

## Naming Constraints

- Avoid making third-party marks the primary product name.
- Use compatibility language for third-party systems:
  - "OpenSearch-compatible", not "mainstack-search" as a product name.
  - "Cassandra-compatible" or "CQL-compatible", not "Cassandra Lite".
  - "Kafka-compatible", not "Kafka Lite", if a stream service is added.
- Keep wasmCloud named as upstream wasmCloud in notices and technical
  provenance, but use a distinct name for any custom host binary.
- If IBM-branded, prefer normal IBM product-family architecture over coined
  names that alter or embed `IBM` itself.

## Key Trademark and Brand Observations

### `mainstack-search`

`mainstack-search` is risky as a public name because it makes `OpenSearch` the
primary product mark. A safer pattern is a distinct product name plus
"OpenSearch-compatible local search service" as descriptive copy.

Source:
<https://opensearch.org/trademark-brand-policy/>

### `mainstack-cql`

`mainstack-cql` is safer than `cassandra-lite` because it does not use
`Cassandra` as the primary mark. It still needs clear descriptive copy:
"Cassandra-compatible CQL/native-protocol server for local development."

Sources:

- <https://www.apache.org/foundation/marks/>
- <https://www.apache.org/foundation/marks/list/>

### wasmCloud

wasmCloud appears suitable to bundle or modify under its open source license
terms, but license permission is separate from trademark permission. If bundled
unchanged, identify it as wasmCloud and preserve notices. If a custom host is
built, give the host a distinct product/component name.

Sources:

- <https://github.com/wasmCloud/wasmCloud>
- <https://www.apache.org/licenses/LICENSE-2.0.txt>
- <https://www.linuxfoundation.org/legal/trademark-usage>
- <https://www.linuxfoundation.org/legal/trademarks>

### IBM and `AIBM`

`AIBM` was rejected. If this is not officially IBM-branded, it directly embeds
the IBM mark. If it is officially IBM-branded, it still reads poorly as a
product name and creates awkward forms such as `IBM AIBM`.

IBM guidance emphasizes proper use of IBM marks, not altering marks, not
creating new IBM logos/product-name forms without approval, and emphasizing the
actual product name when referencing compatibility.

Source:
<https://www.ibm.com/legal/copyright-trademark>

### `Mainstack`

`Mainstack` was rejected as a public product/architecture name because `Mainstack
Framework` already exists in the adjacent event-driven/CQRS/event-sourcing
space, creating avoidable confusion.

Source:
<https://www.axoniq.io/axon-framework>

## Candidate Name Discussion

### Rejected or Lower-Priority

- `mainstack-search`: third-party mark as product name.
- `Cassandra Lite`: third-party mark as product name.
- `Mainstack`: adjacent collision with Mainstack Framework and other Mainstack marks.
- `AIBM`: awkward and too directly modifies/embeds IBM.
- `Qascade`, `Qonduit`, `Qyra`, `Kyra`, `Qivra`, `Qendra`, `Runplane`,
  `TruePlane`, `Baseplane`: quick collision scans found adjacent software,
  AI, platform, or enterprise usage.

### Non-IBM Fallback Candidates

If the product cannot or should not use an IBM/watsonx-aligned name, stronger
fallback candidates discussed were:

- `Kiyu`: short, distinctive, pronounced like "Q", but personal-origin and
  external clearance would still be required.
- `RunWeave`: communicates runtime plus connected enterprise fabric, but
  "weave" is crowded in cloud/AI naming.
- `Bridgeplane`: enterprise architecture feel and implies bridging local to
  production, but more technical and less product-like.

## Current Best Direction

The strongest IBM-native direction discussed was:

**`watsonx.data Q Framework`**

Use `Q Framework` as shorthand only after first use. Avoid `watsonx.q
Framework` unless IBM explicitly wants to create a new top-level `watsonx.*`
pillar. Keeping `Q Framework` under `watsonx.data` fits the goal of driving
consumption of watsonx.data data products and avoids implying a new sibling to
`watsonx.ai`, `watsonx.data`, and `watsonx.governance`.

Recommended public meaning of `Q`: `Query` or `Queryable`. Do not position it
as "Quantum" or as a personal reference.

Possible naming architecture:

| Layer | Name |
| --- | --- |
| Production framework | `watsonx.data Q Framework` |
| Full production profile | `watsonx.data Q Enterprise` |
| Small/workgroup profile | `watsonx.data Q Workgroup` |
| Local development profile | `watsonx.data Q Local` |
| OpenSearch-compatible local search service | `q-search` |
| Cassandra/CQL-compatible local service | `q-cql` |
| Kafka-compatible local stream service | `q-stream` |
| wasmCloud-based runtime host | `q-host` |
| CLI | `q` or `wxdq` |

Example positioning:

> watsonx.data Q Framework helps teams build agent-ready applications that
> consume governed data products through production-compatible local,
> workgroup, and enterprise runtimes.

Component descriptions:

- `q-search`: OpenSearch-compatible local search service.
- `q-cql`: Cassandra-compatible local CQL/native-protocol service.
- `q-stream`: Kafka-compatible local event stream service.
- `q-host`: wasmCloud-based application host.
- `q-local`: bundled local development/workgroup stack.

`q-search`, `q-cql`, and `q-stream` are likely too generic to be strong
standalone marks, but that is acceptable if they remain component names under
the stronger umbrella `watsonx.data Q Framework`.

## IBM Product-Family Fit

This direction appears more coherent than an independent brand because the value
proposition is tied to consumption of `watsonx.data` data products. IBM already
uses `watsonx Orchestrate` for agent building/orchestration, so the name should
avoid suggesting that this product replaces Orchestrate. The clearer lane is:

- `watsonx.data Q Framework`: application/data-consumption framework and
  production-compatible local/workgroup runtime stack for data products.
- `watsonx Orchestrate`: agent orchestration, agent catalog, governance, and
  enterprise automation platform.

Relevant IBM sources:

- <https://www.ibm.com/products/watsonx-orchestrate>
- <https://www.ibm.com/products/watsonx-orchestrate/developers>
- <https://www.ibm.com/docs/en/watsonx/watson-orchestrate/base?topic=agents-introduction-ai>
- <https://www.ibm.com/downloads/documents/us-en/153d3d3aeccfaedb>

## Follow-Up Checklist

Before using any name externally:

- Run IBM internal brand/legal review.
- Confirm whether `watsonx.data Q Framework` is acceptable under IBM naming
  conventions.
- Search IBM internal product and project registries for `Q Framework`,
  `q-search`, `q-cql`, `q-stream`, and `q-local`.
- Check public collisions across USPTO, GitHub, crates.io, npm, Docker Hub,
  GHCR, domains, and major cloud marketplaces.
- Decide whether the CLI should be `q`, `wxdq`, or a longer collision-resistant
  command name.
- Add trademark attribution and compatibility disclaimers to README, docs,
  packaging metadata, and product pages.
