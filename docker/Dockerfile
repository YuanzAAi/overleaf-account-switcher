# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim@sha256:83f487e0a63425e5b4d146fb5e5be574bcbe1b7b843d3ebafdd95eaf7767a7e5 AS web
WORKDIR /web
COPY apps/web/package*.json ./
RUN npm ci
COPY apps/web ./
RUN npm run build

FROM rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS build
RUN apt-get -o Acquire::Retries=3 update && apt-get -o Acquire::Retries=3 install -y --no-install-recommends libdbus-1-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps
COPY --from=web /web/dist ./apps/web/dist
ENV CARGO_BUILD_JOBS=1 CARGO_PROFILE_RELEASE_DEBUG=0
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,id=overleaf-target-${TARGETARCH},target=/src/target \
    cargo build --locked --release -p overleaf-service --bin overleaf-service-api && \
    cp target/release/overleaf-service-api /usr/local/bin/

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171
RUN apt-get -o Acquire::Retries=3 update && apt-get -o Acquire::Retries=3 install -y --no-install-recommends \
    ca-certificates chromium dbus-x11 gnome-keyring libsecret-tools python3 git \
    xvfb x11-utils x11vnc fluxbox feh novnc websockify fonts-noto-cjk tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 overleaf \
    && mkdir -p /data /opt/overleaf && chown overleaf:overleaf /data
COPY --from=build /usr/local/bin/overleaf-service-api /usr/local/bin/
COPY chrome_extension /opt/overleaf/chrome_extension
COPY docker/entrypoint.sh docker/chromium.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/chromium.sh
USER overleaf
WORKDIR /opt/overleaf
ENV DISPLAY=:99 XDG_DATA_HOME=/data XDG_RUNTIME_DIR=/tmp/runtime-overleaf \
    OVERLEAF_SWITCHER_DEPLOYMENT=docker \
    OVERLEAF_SWITCHER_BROWSER_VIEWER_PORT=6080 \
    OVERLEAF_SWITCHER_DATA_DIR=/data \
    OVERLEAF_SWITCHER_TMP_DIR=/tmp/overleaf-account-switcher \
    OVERLEAF_SWITCHER_WORKSPACE_DIR=/opt/overleaf \
    OVERLEAF_SWITCHER_SERVICE_HOST=0.0.0.0 \
    OVERLEAF_SWITCHER_EXTENSION_BRIDGE_HOST=0.0.0.0 \
    OVERLEAF_SWITCHER_CHROME_PATH=/usr/local/bin/chromium.sh
EXPOSE 8765 9876 6080
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
