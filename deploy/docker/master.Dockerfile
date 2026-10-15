# Build from the repository root:
#   docker build -f deploy/docker/master.Dockerfile -t dfsha-master .
# Two stages so the final image carries only the binary (no toolchain).

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY proto ./proto
COPY crates ./crates
RUN cargo build --release -p master

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 dfsha
COPY --from=build /src/target/release/master /usr/local/bin/master
USER dfsha
# gRPC and health check ports (see crates/master/src/config.rs).
EXPOSE 50051 8080
HEALTHCHECK --interval=10s --timeout=3s CMD curl -fs http://localhost:8080/health || exit 1
ENTRYPOINT ["master"]
