FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/opensearch-lite /usr/local/bin/opensearch-lite
EXPOSE 9200
ENTRYPOINT ["opensearch-lite"]
