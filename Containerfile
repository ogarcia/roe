# RoE container: Reticulum (rnsd) with the RoE interface, ready to run.
#
# Build:  podman build -t roe .
# Run:    podman run -d --name roe --network host --cap-add NET_RAW \
#           -e ROE_DEVICE=eth0 -v roe-data:/data roe
#
# See docs/container.md for details.

ARG RNS_VERSION=1.6.0

# Build roe as a static musl binary.
FROM docker.io/library/rust:1-alpine AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin roe

FROM docker.io/library/python:3.13-alpine
ARG RNS_VERSION
LABEL org.opencontainers.image.title="RoE" \
      org.opencontainers.image.description="Reticulum over Ethernet: Reticulum with the RoE interface" \
      org.opencontainers.image.source="https://github.com/ogarcia/roe" \
      org.opencontainers.image.licenses="GPL-3.0-or-later AND MIT"

RUN pip install --no-cache-dir --disable-pip-version-check --root-user-action=ignore \
        "rns==${RNS_VERSION}"
COPY --from=build /src/target/release/roe /usr/local/bin/roe
COPY dist-files/RoeInterface.py /usr/share/roe/RoeInterface.py
COPY container/entrypoint.sh /usr/local/bin/entrypoint.sh

# Reticulum keeps its configuration, identities and paths here. The link
# lets Reticulum's tools (rnstatus, rnprobe...) find it without --config.
RUN mkdir /data && ln -s /data /root/.reticulum
VOLUME /data
ENV PYTHONUNBUFFERED=1

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
