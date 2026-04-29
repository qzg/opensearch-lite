# Driver Examples

OpenSearch Lite uses the normal HTTP OpenSearch endpoint shape. Configure local
clients with no TLS and no authentication unless your wrapper requires explicit
values.

## Python

```python
from opensearchpy import OpenSearch

client = OpenSearch(hosts=[{"host": "127.0.0.1", "port": 9200}], use_ssl=False)
client.indices.create("orders", ignore=400)
client.index(index="orders", id="1", body={"status": "paid", "total": 42})
print(client.count(index="orders", body={"query": {"term": {"status": "paid"}}}))
print(client.search(index="orders", body={"query": {"term": {"status": "paid"}}}))
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

## Java

The repository smoke uses the official `org.opensearch.client:opensearch-java`
client against the local endpoint:

```sh
scripts/run-java-client-smoke.sh
```

## Direct HTTP

```sh
curl -X POST http://127.0.0.1:9200/_bulk -H 'content-type: application/x-ndjson' --data-binary @bulk.ndjson
```
