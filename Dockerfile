FROM node:22-bookworm-slim AS ui

WORKDIR /src/ui
RUN corepack enable && corepack prepare pnpm@9.15.9 --activate
COPY ui/package.json ui/pnpm-lock.yaml ui/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile --ignore-workspace
COPY ui/ ./
RUN pnpm --ignore-workspace run build

FROM rust:bookworm AS builder

ARG SOURCE_REVISION="unknown"
WORKDIR /src

COPY .git/HEAD ./.git/HEAD
COPY .git/refs/heads ./.git/refs/heads

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake golang-go perl clang \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY docs ./docs
COPY src ./src

RUN head="$(cat .git/HEAD)" \
    && case "$head" in \
         'ref: '*) ref="${head#ref: }"; actual_revision="$(cat ".git/${ref}")" ;; \
         *) actual_revision="$head" ;; \
       esac \
    && test "$SOURCE_REVISION" = "$actual_revision" \
    && cargo build --locked --release \
    && cp target/release/raahi /tmp/raahi \
    && strip /tmp/raahi

FROM debian:bookworm-slim

LABEL org.opencontainers.image.source="https://github.com/iamd3vil/raahi" \
      org.opencontainers.image.description="A fast, self-hosted reverse proxy and API gateway" \
      org.opencontainers.image.licenses="GPL-3.0-only"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --create-home raahi

COPY --from=builder /tmp/raahi /usr/local/bin/raahi
COPY --from=ui /src/ui/build /opt/raahi/ui/build

USER raahi
WORKDIR /data

ENV RAAHI_DB="sqlite:///data/raahi.db" \
    RAAHI_UI_DIR="/opt/raahi/ui/build" \
    RUST_LOG="info"

EXPOSE 8080 8443 9080
VOLUME ["/data"]

ENTRYPOINT ["raahi"]
CMD ["--admin-addr", "0.0.0.0:9080"]
