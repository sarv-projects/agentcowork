# AgentCowork headless executor node (P40.1 — spec H33).
#
# Builds the SAME core binary + the Bun-compiled coordinator sidecar and runs
# them in the headless profile (`--headless`): core + B7 scheduler +
# browser/script/office engines, no tray/UI. This is the image the
# BYO-host pack (P40.3) pins on Fly.io / DO / AWS / GCP / Hetzner / Pi.
#
# Hard rule (spec §8): this is a USER-OPERATED image — you build it and run
# it on hardware under YOUR credentials. No founder account, no rented fleet.

# --- stage 1: build the coordinator sidecar (Bun-compiled) ---
FROM oven/bun:1 AS coordinator
WORKDIR /src
COPY packages/coordinator/package.json pnpm-workspace.yaml ./
COPY packages/ /src/packages/
RUN bun install --frozen-lockfile --production || bun install --production
WORKDIR /src/packages/coordinator
RUN bun build --compile ./src/index.ts --external canvas --outfile dist/coordinator

# --- stage 2: build the Rust core ---
FROM rust:1-slim AS core
WORKDIR /src
COPY crates/Cargo.toml crates/Cargo.lock ./crates/
COPY crates/ ./crates/
RUN cargo build --release -p agentcowork-core
# The core binary is what `cargo build` emits for the bin target.
RUN ls target/release/agentcowork-core && cp target/release/agentcowork-core /out/agentcowork-core

# --- stage 3: minimal runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=coordinator /src/packages/coordinator/dist/coordinator /app/coordinator
COPY --from=core /out/agentcowork-core /app/agentcowork-core
# The node's state lives on a user-mounted volume (never in the image).
# New data-home first, legacy fallback alongside: the current binary still
# reads EVERYAIOS_HOME (DEC-053 migration is Core-owned).
ENV AGENTCOWORK_HOME=/data
ENV EVERYAIOS_HOME=/data
VOLUME /data
EXPOSE 47615 # P8.9 E2E sync transport (LAN/Tailscale/WireGuard mesh)
# Headless profile: core + coordinator (B7 scheduler inside). Approvals that
# need Guard-2 park as pending and surface on the control plane.
ENTRYPOINT ["/app/agentcowork-core", "--headless", "--coordinator-bin", "/app/coordinator"]
