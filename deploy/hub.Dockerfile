FROM rust:1.95.0-slim-trixie@sha256:e14e87345b4d5964ddcc3491d27ee046a0f23820f340c3c1e24da6880141f7c0 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY hub hub
COPY agent agent
RUN cargo build --release --locked -p nixyon-hub

FROM debian:trixie-slim@sha256:89c1ea91e897f0fe0a3731595330bc45b8bdef4047464a07116f891a94054b31
RUN useradd --uid 10001 --create-home hub && mkdir -p /data && chown hub /data
COPY --from=build /src/target/release/nixyon-hub /usr/local/bin/nixyon-hub
USER hub
ENTRYPOINT ["nixyon-hub"]
CMD ["serve"]
