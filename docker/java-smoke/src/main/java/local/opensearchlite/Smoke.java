package local.opensearchlite;

import java.net.URI;
import java.util.List;
import java.util.Map;
import org.apache.hc.core5.http.HttpHost;
import org.opensearch.client.opensearch.OpenSearchClient;
import org.opensearch.client.opensearch._types.Refresh;
import org.opensearch.client.opensearch.core.CountResponse;
import org.opensearch.client.opensearch.core.GetResponse;
import org.opensearch.client.opensearch.core.IndexResponse;
import org.opensearch.client.opensearch.core.SearchResponse;
import org.opensearch.client.transport.OpenSearchTransport;
import org.opensearch.client.transport.httpclient5.ApacheHttpClient5TransportBuilder;

public class Smoke {
  private static final String INDEX = "java-smoke";

  public static void main(String[] args) throws Exception {
    URI endpoint =
        URI.create(System.getenv().getOrDefault("OPENSEARCH_URL", "http://127.0.0.1:9200"));
    OpenSearchTransport transport =
        ApacheHttpClient5TransportBuilder.builder(
                new HttpHost(endpoint.getScheme(), endpoint.getHost(), endpoint.getPort()))
            .build();
    OpenSearchClient client = new OpenSearchClient(transport);

    client.info();
    try {
      client.indices().delete(request -> request.index(INDEX));
    } catch (Exception ignored) {
      // Missing cleanup indexes are expected on a fresh smoke run.
    }
    client.indices().create(request -> request.index(INDEX));

    IndexResponse created =
        client.index(
            request ->
                request
                    .index(INDEX)
                    .id("1")
                    .document(Map.of("customer_id", "c1", "status", "paid", "total", 42.5))
                    .refresh(Refresh.True));
    require(List.of("created", "updated").contains(created.result().jsonValue()), "index result");

    GetResponse<Map> doc = client.get(request -> request.index(INDEX).id("1"), Map.class);
    require(doc.found(), "document should exist");
    require("c1".equals(doc.source().get("customer_id")), "document source");

    CountResponse count =
        client.count(
            request ->
                request
                    .index(INDEX)
                    .query(
                        query ->
                            query.term(
                                term ->
                                    term.field("customer_id")
                                        .value(value -> value.stringValue("c1")))));
    require(count.count() == 1, "count result");

    SearchResponse<Map> search =
        client.search(
            request ->
                request
                    .index(INDEX)
                    .query(query -> query.matchAll(matchAll -> matchAll)),
            Map.class);
    require(search.hits().total().value() == 1, "search total");

    System.out.println("Java OpenSearch client smoke passed");
  }

  private static void require(boolean condition, String label) {
    if (!condition) {
      throw new IllegalStateException(label);
    }
  }
}
