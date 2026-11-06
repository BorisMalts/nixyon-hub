ARG BASE=nixyon-android:dev

FROM rust:1.95.0-slim-trixie@sha256:e14e87345b4d5964ddcc3491d27ee046a0f23820f340c3c1e24da6880141f7c0 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY hub hub
COPY agent agent
RUN cargo build --release --locked -p nixyon-agent

FROM ${BASE}
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends git openssh-client bash procps less nano \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/nixyon-agent /usr/local/bin/nixyon-agent
COPY deploy/runner/nixyon-live-eval /usr/local/bin/nixyon-live-eval
COPY deploy/github_known_hosts /etc/ssh/ssh_known_hosts
RUN mkdir -p /work /artifacts /locks /models-mirror /home/builder/.android && chown -R builder /work /artifacts /locks /models-mirror /home/builder
USER builder
WORKDIR /work
ENV NIXYON_AGENT_LISTEN=0.0.0.0:7700 \
    NIXYON_REPO_DIR=/work/repo \
    NIXYON_ARTIFACTS_DIR=/artifacts \
    NIXYON_AGENT_TOKEN_FILE=/run/secrets/agent_token \
    GIT_SSH_COMMAND="ssh -i /run/secrets/deploy_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes"
ENTRYPOINT ["nixyon-agent"]
