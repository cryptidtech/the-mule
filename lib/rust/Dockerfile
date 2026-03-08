FROM rust:latest AS builder
WORKDIR /build
COPY Cargo.toml ./
COPY src/ ./src/
RUN cargo build --release --bin reference-peer

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends redis-tools && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/reference-peer /usr/local/bin/reference-peer
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
