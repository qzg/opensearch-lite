package local.opensearchlite;

import java.io.InputStream;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.SecureRandom;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.Map;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;
import org.apache.hc.client5.http.auth.AuthScope;
import org.apache.hc.client5.http.auth.UsernamePasswordCredentials;
import org.apache.hc.client5.http.impl.auth.BasicCredentialsProvider;
import org.apache.hc.client5.http.impl.nio.PoolingAsyncClientConnectionManager;
import org.apache.hc.client5.http.impl.nio.PoolingAsyncClientConnectionManagerBuilder;
import org.apache.hc.client5.http.ssl.ClientTlsStrategyBuilder;
import org.apache.hc.core5.function.Factory;
import org.apache.hc.core5.http.HttpHost;
import org.apache.hc.core5.http.nio.ssl.TlsStrategy;
import org.apache.hc.core5.reactor.ssl.TlsDetails;
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
    HttpHost host = new HttpHost(endpoint.getScheme(), endpoint.getHost(), endpoint.getPort());
    ApacheHttpClient5TransportBuilder builder = ApacheHttpClient5TransportBuilder.builder(host);
    BasicCredentialsProvider credentials = credentialsProvider(host);
    SSLContext sslContext = sslContext();
    if (credentials != null || sslContext != null) {
      builder.setHttpClientConfigCallback(
          httpClientBuilder -> {
            if (credentials != null) {
              httpClientBuilder.setDefaultCredentialsProvider(credentials);
            }
            if (sslContext != null) {
              final TlsStrategy tlsStrategy =
                  ClientTlsStrategyBuilder.create()
                      .setSslContext(sslContext)
                      .setTlsDetailsFactory(
                          new Factory<SSLEngine, TlsDetails>() {
                            @Override
                            public TlsDetails create(final SSLEngine sslEngine) {
                              return new TlsDetails(
                                  sslEngine.getSession(), sslEngine.getApplicationProtocol());
                            }
                          })
                      .build();
              final PoolingAsyncClientConnectionManager connectionManager =
                  PoolingAsyncClientConnectionManagerBuilder.create()
                      .setTlsStrategy(tlsStrategy)
                      .build();
              httpClientBuilder.setConnectionManager(connectionManager);
            }
            return httpClientBuilder;
          });
    }
    OpenSearchTransport transport = builder.build();
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

  private static BasicCredentialsProvider credentialsProvider(HttpHost host) {
    String username = System.getenv("OPENSEARCH_USERNAME");
    String password = System.getenv("OPENSEARCH_PASSWORD");
    if ((username == null || username.isEmpty()) && (password == null || password.isEmpty())) {
      return null;
    }
    BasicCredentialsProvider credentials = new BasicCredentialsProvider();
    credentials.setCredentials(
        new AuthScope(host),
        new UsernamePasswordCredentials(
            username == null ? "" : username, (password == null ? "" : password).toCharArray()));
    return credentials;
  }

  private static SSLContext sslContext() throws Exception {
    String verify = System.getenv().getOrDefault("OPENSEARCH_VERIFY_CERTS", "true");
    if (List.of("0", "false", "no").contains(verify.toLowerCase())) {
      return insecureTrustAllContext();
    }
    String caPath = System.getenv("OPENSEARCH_CA_CERT");
    if (caPath == null || caPath.isBlank()) {
      return null;
    }

    CertificateFactory factory = CertificateFactory.getInstance("X.509");
    Certificate certificate;
    try (InputStream input = Files.newInputStream(Path.of(caPath))) {
      certificate = factory.generateCertificate(input);
    }
    KeyStore trustStore = KeyStore.getInstance(KeyStore.getDefaultType());
    trustStore.load(null, null);
    trustStore.setCertificateEntry("opensearch-lite-ca", certificate);
    TrustManagerFactory trustManagerFactory =
        TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
    trustManagerFactory.init(trustStore);
    SSLContext context = SSLContext.getInstance("TLS");
    context.init(null, trustManagerFactory.getTrustManagers(), null);
    return context;
  }

  private static SSLContext insecureTrustAllContext() throws Exception {
    TrustManager[] trustAll =
        new TrustManager[] {
          new X509TrustManager() {
            @Override
            public void checkClientTrusted(X509Certificate[] chain, String authType) {}

            @Override
            public void checkServerTrusted(X509Certificate[] chain, String authType) {}

            @Override
            public X509Certificate[] getAcceptedIssuers() {
              return new X509Certificate[0];
            }
          }
        };
    SSLContext context = SSLContext.getInstance("TLS");
    context.init(null, trustAll, new SecureRandom());
    return context;
  }
}
