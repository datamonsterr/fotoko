# syntax=docker/dockerfile:1
# The Rust parts of the local stack. One build stage compiles every binary, one small runtime
# stage per service (select with `target:` in docker-compose.yml). Build context: the repo root.

ARG RUST_VERSION=1.99

FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml clippy.toml ./
COPY crates ./crates
# The registry and target dir live in cache mounts, so a rebuild only recompiles what changed.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
        -p fotoko-e2e -p fotoko-api -p fotoko-pi-agent --features fotoko-api/local \
        --bin fotoko-sim-broker --bin fotoko-payos-mock --bin fotoko-local --bin fotoko-pi-agent \
    && mkdir /out \
    && cp target/release/fotoko-sim-broker target/release/fotoko-payos-mock \
          target/release/fotoko-local target/release/fotoko-pi-agent /out/

FROM debian:bookworm-slim AS runtime
RUN useradd --system --create-home --uid 10001 fotoko
USER fotoko

FROM runtime AS broker
COPY --from=build /out/fotoko-sim-broker /usr/local/bin/
ENV FOTOKO_BROKER_ADDR=0.0.0.0:1883
EXPOSE 1883
ENTRYPOINT ["fotoko-sim-broker"]

FROM runtime AS payos-mock
COPY --from=build /out/fotoko-payos-mock /usr/local/bin/
ENV PAYOS_MOCK_ADDR=0.0.0.0:8082
EXPOSE 8082
ENTRYPOINT ["fotoko-payos-mock"]

FROM runtime AS api
COPY --from=build /out/fotoko-local /usr/local/bin/
ENV FOTOKO_LOCAL_ADDR=0.0.0.0:8787 FOTOKO_LOCAL_DIR=/home/fotoko/objects
EXPOSE 8787
ENTRYPOINT ["fotoko-local"]

FROM runtime AS pi-agent
USER root
RUN mkdir -p /var/lib/fotoko/sim-out /var/lib/fotoko/state \
    && chown -R fotoko /var/lib/fotoko
USER fotoko
COPY --from=build /out/fotoko-pi-agent /usr/local/bin/
ENTRYPOINT ["fotoko-pi-agent"]
CMD ["--config", "/etc/fotoko/agent.toml"]
