# The whole product as one image: the server, and the two apps it serves.
#
# A shop that self-hosts should run one image and one database. Anything else is
# a web server to configure as well, on a machine in a shop, by somebody whose
# job is selling rice.
#
# Three stages, because the apps need a Rust toolchain to build the wasm and a
# Node one to bundle it, and the thing that ships needs neither.

# --- the wasm and the two apps ------------------------------------------------
FROM rust:1-bookworm AS apps
WORKDIR /src
RUN curl -sSf https://rustwasm.github.io/wasm-pack/installer/init.sh | sh \
    && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY core core
COPY bindings bindings
COPY ffi ffi
COPY server server
COPY apps apps
RUN cd bindings && wasm-pack build --target web --release --out-dir ../target/pkg
RUN cp -r target/pkg apps/till-web/public/pkg \
    && cp -r target/pkg apps/admin/public/pkg \
    && cd apps/till-web && npm ci --no-audit --no-fund && npm run build \
    && cd ../admin && npm ci --no-audit --no-fund && npm run build \
    && cp -r /src/apps/admin/dist /src/apps/till-web/dist/admin

# --- the server ---------------------------------------------------------------
FROM rust:1-bookworm AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY core core
COPY bindings bindings
COPY ffi ffi
COPY server server
RUN cargo build --release -p openpos-server

# --- what ships ---------------------------------------------------------------
FROM debian:bookworm-slim
# The migrations are read at build time by sqlx's macro, so nothing here needs
# the source. Certificates are for a managed Postgres over TLS.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 openpos
COPY --from=server /src/target/release/openpos-server /usr/local/bin/openpos-server
COPY --from=apps /src/apps/till-web/dist /srv/apps
# Where the backup sidecar writes. Created here and owned by the user the image
# runs as, because a named volume takes its ownership from the path in the
# image the first time it is mounted: without this, the sidecar starts, runs
# nightly, and cannot write a single file.
RUN mkdir -p /backups && chown openpos:openpos /backups
USER openpos
ENV OPENPOS_LISTEN=0.0.0.0:8080 \
    OPENPOS_APPS=/srv/apps
EXPOSE 8080
# No shell in between, so a stop signal reaches the server and it finishes the
# requests it is holding rather than dropping a sync mid-batch.
ENTRYPOINT ["/usr/local/bin/openpos-server"]
