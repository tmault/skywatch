# syntax=docker/dockerfile:1.7
#
# Skywatch: multi-stage build. Base images are pinned by digest for
# reproducibility; refresh deliberately (see README "Updating base images").

# ---- build stage -----------------------------------------------------------
FROM rust:1.100-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build

# The server resolves its static assets at CARGO_MANIFEST_DIR/../web (baked in
# at compile time), and embeds scripts/t3_bridge.py with include_str!. Keep the
# repository layout under /app so both resolve identically in the runtime image.
WORKDIR /app/server

# Dependency layer: build against stub sources so it is cached until the
# manifest or lockfile changes.
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/skywatch-server target/release/deps/skywatch_server* target/release/.fingerprint/skywatch-server-*

COPY server/src ./src
COPY scripts /app/scripts
# Source mtimes can predate the stub build; force cargo to rebuild the crate.
RUN touch src/main.rs && cargo build --release --locked

# ---- runtime stage ---------------------------------------------------------
FROM debian:bookworm-slim@sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e AS runtime

# python3 (full stdlib: json, urllib): runs the embedded T3 MCP bridge and the healthcheck. T3's projection database is
# read with the SQLite library bundled into skywatch-server, so no sqlite3 binary is needed.
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG SKYWATCH_UID=1000
ARG SKYWATCH_GID=1000
RUN groupadd --gid "${SKYWATCH_GID}" skywatch \
    && useradd --uid "${SKYWATCH_UID}" --gid "${SKYWATCH_GID}" --home-dir /home/skywatch --create-home --shell /usr/sbin/nologin skywatch

COPY --from=build /app/server/target/release/skywatch-server /usr/local/bin/skywatch-server
COPY web /app/web
# The compiled-in asset path is /app/server/../web, which only resolves if
# /app/server exists; it holds no files.
RUN mkdir -p /app/server

# The adapter derives the T3 database path from $HOME (~/.t3/userdata/statev2.sqlite).
# Live mounts are provided by docker-compose.live.yml; nothing is baked in.
ENV HOME=/home/skywatch \
    SKYWATCH_PORT=8422 \
    SKYWATCH_T3=0 \
    RUST_LOG=info

USER skywatch
WORKDIR /home/skywatch
EXPOSE 8422

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["python3", "-c", "import os,urllib.request;urllib.request.urlopen('http://127.0.0.1:%s/health' % os.environ.get('SKYWATCH_PORT','8422'), timeout=3)"]

ENTRYPOINT ["skywatch-server"]
