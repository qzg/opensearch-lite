FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/mainstack-search /usr/local/bin/mainstack-search
RUN groupadd --system mainstack-search \
    && useradd --system --gid mainstack-search --home-dir /var/lib/mainstack-search mainstack-search \
    && mkdir -p /var/lib/mainstack-search \
    && chown -R mainstack-search:mainstack-search /var/lib/mainstack-search
EXPOSE 9200
USER mainstack-search
WORKDIR /var/lib/mainstack-search
ENTRYPOINT ["mainstack-search"]
