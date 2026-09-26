# syntax=docker/dockerfile:1
# Usage: docker build -t ostra . (run from the repo root)
# BuildKit cache mounts keep the cargo registry, target dir, and npm cache across builds.

FROM ubuntu:26.04 AS web
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
    && curl -fsSL https://deb.nodesource.com/setup_24.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm \
    node --version \
    && npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run -s build

FROM ubuntu:26.04 AS rust
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gnupg build-essential \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:${PATH}
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none \
    && rustup --version
WORKDIR /src
COPY rust-toolchain.toml ./
RUN rustup toolchain install \
    && rustc --version
COPY Cargo.toml Cargo.lock ./
COPY .cargo/ .cargo/
COPY crates/ crates/
COPY tests/ tests/
COPY assets/ assets/
COPY --from=web /src/web/dist web/dist
# target/ is a cache mount and vanishes after this step, so the binary is copied out of it.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p ostra-server \
    && cp target/release/ostra /usr/local/bin/ostra

FROM ubuntu:26.04 AS runtime
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git openssh-client curl bubblewrap \
    && rm -rf /var/lib/apt/lists/*
# Fixed UID/GID 1000 so a bind-mounted host directory owned by the common first
# non-root Linux user (1000) is writable without a runtime chown.
RUN if getent passwd 1000 >/dev/null; then userdel --remove "$(getent passwd 1000 | cut -d: -f1)"; fi \
    && if getent group 1000 >/dev/null; then groupdel "$(getent group 1000 | cut -d: -f1)"; fi \
    && groupadd --gid 1000 ostra \
    && useradd --uid 1000 --gid 1000 --create-home --home-dir /home/ostra --shell /bin/bash ostra \
    && mkdir -p /data /config \
    && chown 1000:1000 /data /config
ENV OSTRA_DATA_DIR=/data OSTRA_CONFIG=/config/config.toml
COPY --from=rust /usr/local/bin/ostra /usr/local/bin/ostra
# 7878 is Ostra's default serve port (crates/ostra-server), matched by CMD's --port below.
EXPOSE 7878
STOPSIGNAL SIGINT
USER 1000:1000
ENTRYPOINT ["ostra"]
CMD ["--bind", "0.0.0.0", "--port", "7878", "--no-open"]
