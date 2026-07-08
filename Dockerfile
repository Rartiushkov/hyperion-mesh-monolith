# Hyperion Mesh — production backend container
# Builds the RAMAN kernel + raw ingress gateway and exposes TCP/UDP 8081.

# ---------------------------------------------------------------------------
# Stage 1: Build
# ---------------------------------------------------------------------------
FROM rust:1.79-slim-bookworm AS builder

WORKDIR /app

# Install system dependencies required by tonic-build, aws-lc-sys, and openssl.
RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy source and dependency manifests first to leverage Docker cache.
COPY Cargo.toml Cargo.lock build.rs ./
COPY raman-core ./raman-core
COPY proto ./proto
COPY src ./src

# Build the production binaries.
RUN cargo build --release

# ---------------------------------------------------------------------------
# Stage 2: Runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy compiled binaries.
COPY --from=builder /app/target/release/hyperion_raw_ingress /usr/local/bin/hyperion_raw_ingress
COPY --from=builder /app/target/release/hyperion_grpc_server /usr/local/bin/hyperion_grpc_server

# Default env vars (override at runtime with real secrets).
ENV RUST_LOG=info
ENV HYPERION_RAW_ADDR=0.0.0.0:8081

EXPOSE 8081/tcp
EXPOSE 8081/udp
EXPOSE 8080/tcp

# Run the raw ingress gateway by default.
CMD ["hyperion_raw_ingress"]
