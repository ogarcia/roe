# RoE container: Reticulum (rnsd) with the RoE interface, ready to run.
#
# roe is not compiled here: the image takes the static musl binary that CI
# builds and tests, from container/roe. To build the image locally, put the
# binary there first:
#
#   cargo build --release --target "$(uname -m)-unknown-linux-musl"
#   cp "target/$(uname -m)-unknown-linux-musl/release/roe" container/roe
#   podman build -t roe .
#
# Run:  podman run -d --name roe --network host --cap-add NET_RAW \
#         -e ROE_DEVICE=eth0 -v roe-data:/data roe
#
# See docs/container.md for details.

FROM docker.io/library/python:3.13-alpine

ARG RNS_VERSION=1.6.0
LABEL org.opencontainers.image.title="RoE" \
      org.opencontainers.image.description="Reticulum over Ethernet: Reticulum with the RoE interface" \
      org.opencontainers.image.source="https://github.com/ogarcia/roe" \
      org.opencontainers.image.licenses="GPL-3.0-or-later AND MIT"

RUN pip install --no-cache-dir --disable-pip-version-check --root-user-action=ignore \
        "rns==${RNS_VERSION}"
COPY --chmod=755 container/roe /usr/bin/roe
COPY dist-files/RoeInterface.py /usr/share/roe/RoeInterface.py
COPY container/entrypoint.sh /usr/local/bin/entrypoint.sh

# Reticulum keeps its configuration, identities and paths here. The link
# lets Reticulum's tools (rnstatus, rnprobe...) find it without --config.
RUN mkdir /data && ln -s /data /root/.reticulum
VOLUME /data
ENV PYTHONUNBUFFERED=1

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
