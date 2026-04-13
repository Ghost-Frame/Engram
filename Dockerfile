# syntax=docker/dockerfile:1

# =============================================================================
# Stage 1 -- builder
# Compiles engram-server and engram-cli in release mode.
# SQLCipher is vendored at compile time via the "sqlcipher" feature so no
# system libsqlcipher is needed at runtime.
# =============================================================================
FROM rust:1.80-bookworm AS builder

WORKDIR /build

# Install build-time deps needed by vendored SQLCipher and OpenSSL bindings.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    clang \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy the full workspace so Cargo can resolve the dependency graph.
COPY . .

# Build only the two required binaries; the rest of the workspace is skipped.
RUN cargo build --release -p engram-server -p engram-cli

# =============================================================================
# Stage 2 -- runtime
# Minimal Debian image with only the libraries the binaries actually dlopen.
# =============================================================================
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.source="https://github.com/Ghost-Frame/Engram-rust" \
      org.opencontainers.image.description="Engram memory server -- personal knowledge graph and semantic memory store" \
      org.opencontainers.image.licenses="Elastic-2.0"

# Install runtime dependencies:
#   libssl3     -- required by reqwest (native-tls)
#   ca-certificates -- required for outbound HTTPS calls
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a dedicated non-root user for running the server.
RUN groupadd --system --gid 1000 engram \
    && useradd --system --uid 1000 --gid engram --no-create-home --shell /sbin/nologin engram

# Persistent data lives here.  A named volume or bind-mount should be attached.
RUN mkdir -p /data && chown engram:engram /data

COPY --from=builder /build/target/release/engram-server /usr/local/bin/engram-server
COPY --from=builder /build/target/release/engram-cli     /usr/local/bin/engram-cli

RUN chmod 755 /usr/local/bin/engram-server /usr/local/bin/engram-cli

USER engram

# Environment -- bind to all interfaces inside the container.
ENV ENGRAM_HOST=0.0.0.0
ENV ENGRAM_DATA_DIR=/data
ENV ENGRAM_DB_PATH=/data/engram.db

VOLUME ["/data"]

EXPOSE 4200

CMD ["/usr/local/bin/engram-server"]
