ARG BASE=nixyon-android:dev

FROM ${BASE}
USER root
ARG RUNNER_VERSION=2.338.2
ARG RUNNER_SHA256=baebf204307bac6193480852732f0aba2ae97b6c5597c244f4d5558390147c40
RUN apt-get update \
    && apt-get install -y --no-install-recommends git openssh-client bash procps python3 curl ca-certificates gzip \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/runner.tar.gz \
        "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz" \
    && echo "${RUNNER_SHA256}  /tmp/runner.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/actions-runner \
    && tar -xzf /tmp/runner.tar.gz -C /opt/actions-runner \
    && rm /tmp/runner.tar.gz \
    && echo "${RUNNER_VERSION}" > /opt/actions-runner/.nixyon-version
COPY deploy/runner/nixyon-ci deploy/runner/nixyon-runner deploy/runner/nixyon-runner-register /usr/local/bin/
COPY deploy/github_known_hosts /etc/ssh/ssh_known_hosts
RUN mkdir -p /runner /ci-reports /locks /cache/ci /home/builder/.android \
    && chown -R builder /runner /ci-reports /locks /cache /home/builder
USER builder
WORKDIR /runner
ENV NIXYON_CI_REPORTS=/ci-reports \
    NIXYON_HEAVY_LOCK=/locks/heavy.d
ENTRYPOINT ["nixyon-runner"]
