# Build the Rust core service (agentd). Build context: ./backend-rs
FROM rust:1-bookworm AS build
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/target/release/agentd /usr/local/bin/agentd
ENV AGENT_DEBUG_HOST=0.0.0.0 \
    AGENT_DEBUG_DATA_DIR=/data \
    AGENT_CORE_PORT=8003
VOLUME ["/data"]
EXPOSE 8003
CMD ["agentd"]
