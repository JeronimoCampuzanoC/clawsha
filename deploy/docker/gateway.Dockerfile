# Build from the repository root:
#   docker build -f deploy/docker/gateway.Dockerfile \
#     -t samuelmarti22/clawsha-gateway:latest .
# The final image contains only the gateway binary and runtime certificates.

FROM rust:1-slim-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY proto ./proto
COPY crates ./crates

RUN cargo build --release -p gateway

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home gateway

COPY --from=builder /app/target/release/gateway /usr/local/bin/gateway

USER gateway

ENV GATEWAY_ADDR=0.0.0.0:3000
EXPOSE 3000

ENTRYPOINT ["gateway"]
