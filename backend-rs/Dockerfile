# Build agentd from the repository root so sibling moonlit path deps resolve:
#   docker compose build agentd
#   docker build -f backend-rs/Dockerfile .
FROM rust:1-bookworm AS build
WORKDIR /app/backend-rs
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY backend-rs/Cargo.toml backend-rs/Cargo.lock ./
COPY backend-rs/crates ./crates
COPY backend-rs/bins ./bins
COPY rust/Cargo.toml ../rust/Cargo.toml
COPY rust/crates/moonlit-core ../rust/crates/moonlit-core
COPY rust/crates/moonlit-doccore ../rust/crates/moonlit-doccore
COPY rust/crates/moonlit-compile ../rust/crates/moonlit-compile
RUN cargo build --release -p agentd

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/backend-rs/target/release/agentd /usr/local/bin/agentd
ENV AGENT_DEBUG_HOST=0.0.0.0 \
    AGENT_DEBUG_DATA_DIR=/data \
    AGENT_CORE_PORT=8003
VOLUME ["/data"]
EXPOSE 8003
CMD ["agentd"]
