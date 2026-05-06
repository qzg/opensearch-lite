# Driver Examples

mainstack-search uses the normal HTTP OpenSearch endpoint shape. Configure local
clients with no TLS and no authentication unless your wrapper requires explicit
values. For workgroup deployments, use HTTPS, Basic auth, and CA trust just as
you would with a secured OpenSearch cluster.

## Python

```python
from opensearchpy import OpenSearch

client = OpenSearch(hosts=[{"host": "127.0.0.1", "port": 9200}], use_ssl=False)
client.indices.create("orders", ignore=400)
client.index(index="orders", id="1", body={"status": "paid", "total": 42})
print(client.count(index="orders", body={"query": {"term": {"status": "paid"}}}))
print(client.search(index="orders", body={"query": {"term": {"status": "paid"}}}))
```

Secured:

```python
from opensearchpy import OpenSearch

client = OpenSearch(
    hosts=["https://localhost:9200"],
    http_auth=("alice", password_from_secret_store),
    use_ssl=True,
    verify_certs=True,
    ca_certs="/run/mainstack-search/tls/ca.crt",
)
```

## JavaScript

```javascript
import { Client } from "@opensearch-project/opensearch";

const client = new Client({ node: "http://127.0.0.1:9200" });
await client.indices.create({ index: "orders" });
await client.index({ index: "orders", id: "1", body: { status: "paid" } });
console.log(await client.count({ index: "orders", body: { query: { term: { status: "paid" } } } }));
console.log(await client.search({ index: "orders", body: { query: { match_all: {} } } }));
```

Secured:

```javascript
import fs from "node:fs";
import { Client } from "@opensearch-project/opensearch";

const client = new Client({
  node: "https://localhost:9200",
  auth: { username: "alice", password: passwordFromSecretStore },
  ssl: { ca: fs.readFileSync("/run/mainstack-search/tls/ca.crt") },
});
```

## Java

The repository smoke uses the official `org.opensearch.client:opensearch-java`
client against the local endpoint:

```sh
scripts/run-java-client-smoke.sh
```

The smoke honors `OPENSEARCH_URL`, `OPENSEARCH_USERNAME`,
`OPENSEARCH_PASSWORD`, and `OPENSEARCH_CA_CERT`. Set
`MAINSTACK_SEARCH_SECURE_SMOKE=1` to start a temporary HTTPS/auth server with
generated local fixtures.

## Direct HTTP

```sh
curl -X POST http://127.0.0.1:9200/_bulk -H 'content-type: application/x-ndjson' --data-binary @bulk.ndjson
```

Secured:

```sh
curl --cacert /run/mainstack-search/tls/ca.crt \
  -u "${OPENSEARCH_USERNAME}:${OPENSEARCH_PASSWORD}" \
  https://localhost:9200/
```
