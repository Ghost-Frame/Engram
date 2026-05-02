# syntax=docker/dockerfile:1

# =============================================================================
# Stage 1 -- builder
# Compiles kleos-server and kleos-cli in release mode.
# SQLCipher is vendored at compile time via the "sqlcipher" feature so no
# system libsqlcipher is needed at runtime.
# =============================================================================
FROM rust:1.94-bookworm AS builder

WORKDIR /build

# Install build-time deps needed by vendored SQLCipher and OpenSSL bindings.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    clang \
    protobuf-compiler \
    libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy the full workspace so Cargo can resolve the dependency graph.
COPY . .

# Build only the two required binaries; the rest of the workspace is skipped.
RUN cargo build --release -p kleos-server -p kleos-cli

# =============================================================================
# Stage 2 -- runtime
# Minimal Debian image with only the libraries the binaries actually dlopen.
# =============================================================================
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.source="https://github.com/Ghost-Frame/Kleos" \
      org.opencontainers.image.description="Kleos memory server (formerly Engram) -- personal knowledge graph and semantic memory store" \
      org.opencontainers.image.licenses="Elastic-2.0"

# Install runtime dependencies:
#   libssl3     -- required by reqwest (native-tls)
#   ca-certificates -- required for outbound HTTPS calls
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a dedicated non-root user for running the server.
RUN groupadd --system --gid 1000 kleos \
    && useradd --system --uid 1000 --gid kleos --no-create-home --shell /sbin/nologin kleos

# Persistent data lives here.  A named volume or bind-mount should be attached.
RUN mkdir -p /data && chown kleos:kleos /data

COPY --from=builder /build/target/release/kleos-server /usr/local/bin/kleos-server
COPY --from=builder /build/target/release/kleos-cli     /usr/local/bin/kleos-cli

RUN chmod 755 /usr/local/bin/kleos-server /usr/local/bin/kleos-cli

# Legacy aliases for backward compatibility.
RUN ln -s /usr/local/bin/kleos-server /usr/local/bin/engram-server \
    && ln -s /usr/local/bin/kleos-cli /usr/local/bin/engram-cli

USER kleos

# Environment -- bind to all interfaces inside the container.
# KLEOS_* vars are preferred. The env shim falls back to ENGRAM_* automatically.
ENV KLEOS_HOST=0.0.0.0
ENV KLEOS_DATA_DIR=/data
ENV KLEOS_DB_PATH=/data/kleos.db

VOLUME ["/data"]

EXPOSE 4200

CMD ["/usr/local/bin/kleos-server"]
