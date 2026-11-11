# Kestrel Dockerfile
# Multi-stage build for minimal production image

# Build stage
FROM rust:1.97-bookworm AS builder

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build dependencies first (cached layer)
RUN mkdir -p crates/kestrel-core/src && echo "// dummy" > crates/kestrel-core/src/lib.rs
RUN mkdir -p crates/kestrel-venues/src && echo "// dummy" > crates/kestrel-venues/src/lib.rs
RUN mkdir -p crates/kestrel-store/src && echo "// dummy" > crates/kestrel-store/src/lib.rs
RUN mkdir -p crates/kestrel-ingest/src && echo "// dummy" > crates/kestrel-ingest/src/lib.rs
RUN mkdir -p crates/kestrel-state/src && echo "// dummy" > crates/kestrel-state/src/lib.rs
RUN mkdir -p crates/kestrel-safety/src && echo "// dummy" > crates/kestrel-safety/src/lib.rs
RUN mkdir -p crates/kestrel-scorer/src && echo "// dummy" > crates/kestrel-scorer/src/lib.rs
RUN mkdir -p crates/kestrel-pnl/src && echo "// dummy" > crates/kestrel-pnl/src/lib.rs
RUN mkdir -p crates/kestrel-exec/src && echo "// dummy" > crates/kestrel-exec/src/lib.rs
RUN mkdir -p crates/kestrel-strategy/src && echo "// dummy" > crates/kestrel-strategy/src/lib.rs
RUN mkdir -p crates/kestrel-risk/src && echo "// dummy" > crates/kestrel-risk/src/lib.rs
RUN mkdir -p crates/kestrel-portfolio/src && echo "// dummy" > crates/kestrel-portfolio/src/lib.rs
RUN mkdir -p crates/kestrel-engine/src && echo "// dummy" > crates/kestrel-engine/src/lib.rs
RUN mkdir -p crates/kestrel-bin/src && echo "// dummy" > crates/kestrel-bin/src/main.rs

# Build dependencies
RUN cargo build --workspace --release --bin kestrel 2>/dev/null || true

# Copy actual source
COPY crates/kestrel-core/src ./crates/kestrel-core/src
COPY crates/kestrel-venues/src ./crates/kestrel-venues/src
COPY crates/kestrel-store/src ./crates/kestrel-store/src
COPY crates/kestrel-ingest/src ./crates/kestrel-ingest/src
COPY crates/kestrel-state/src ./crates/kestrel-state/src
COPY crates/kestrel-safety/src ./crates/kestrel-safety/src
COPY crates/kestrel-scorer/src ./crates/kestrel-scorer/src
COPY crates/kestrel-pnl/src ./crates/kestrel-pnl/src
COPY crates/kestrel-exec/src ./crates/kestrel-exec/src
COPY crates/kestrel-strategy/src ./crates/kestrel-strategy/src
COPY crates/kestrel-risk/src ./crates/kestrel-risk/src
COPY crates/kestrel-portfolio/src ./crates/kestrel-portfolio/src
COPY crates/kestrel-engine/src ./crates/kestrel-engine/src
COPY crates/kestrel-bin/src ./crates/kestrel-bin/src

# Build final binary
RUN cargo build --workspace --release --bin kestrel

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/kestrel /usr/local/bin/kestrel

# Create non-root user
RUN useradd -r -s /bin/false -u 1000 kestrel
USER kestrel

# Expose metrics port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD kestrel --help >/dev/null 2>&1 || exit 1

# Entry point
ENTRYPOINT ["kestrel"]
CMD ["run", "--mode", "paper"]