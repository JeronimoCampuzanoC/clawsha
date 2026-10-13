# Build from the repository root:
#   docker build -f deploy/docker/worker.Dockerfile -t dfsha-worker .
# Two stages so the final image carries only the binary (no toolchain).

# ---------- Stage 1: build ----------
FROM rust:1-slim-bookworm AS builder

# protoc is needed by build.rs to generate the gRPC code.
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# The workspace root, the shared contract and every crate.
COPY Cargo.toml Cargo.lock ./
COPY proto ./proto
COPY crates ./crates

RUN cargo build --release -p worker

# ---------- Stage 2: runtime ----------
FROM debian:bookworm-slim

# Run as a normal user, not as root.
RUN useradd --system --uid 10001 --no-create-home worker \
    && mkdir /data \
    && chown worker:worker /data

COPY --from=builder /app/target/release/worker /usr/local/bin/worker

USER worker

ENV WORKER_DATA_DIR=/data
EXPOSE 50052
VOLUME /data

# Exec form: the worker is PID 1 and receives SIGTERM directly.
ENTRYPOINT ["worker"]
