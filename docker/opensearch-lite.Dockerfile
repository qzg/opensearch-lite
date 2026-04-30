FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/opensearch-lite /usr/local/bin/opensearch-lite
RUN groupadd --system opensearch-lite \
    && useradd --system --gid opensearch-lite --home-dir /var/lib/opensearch-lite opensearch-lite \
    && mkdir -p /var/lib/opensearch-lite \
    && chown -R opensearch-lite:opensearch-lite /var/lib/opensearch-lite
EXPOSE 9200
USER opensearch-lite
WORKDIR /var/lib/opensearch-lite
ENTRYPOINT ["opensearch-lite"]
