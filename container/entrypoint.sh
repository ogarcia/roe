#!/bin/sh
# Entrypoint of the RoE container.
#
# Uses /data as Reticulum's config directory, links RoeInterface.py into it
# and, on first run, writes a config with an RoE interface on $ROE_DEVICE.
# Then runs rnsd in the foreground, logging to standard output. Any other
# command (rnstatus, rnprobe, sh...) is run instead; the image links
# ~/.reticulum to /data, so Reticulum's tools find the same config.
#
# Environment:
#   ROE_DEVICE      network interface for RoE (default: eth0)
#   ROE_TRANSPORT   Yes to make this node a transport node (default: No)
set -eu

CONFIG_DIR=/data

mkdir -p "$CONFIG_DIR/interfaces"
ln -sf /usr/share/roe/RoeInterface.py "$CONFIG_DIR/interfaces/RoeInterface.py"

if [ ! -f "$CONFIG_DIR/config" ]; then
    cat > "$CONFIG_DIR/config" <<EOF
# Written by the RoE container on first run. Edit freely: it is not
# overwritten while it exists.
[reticulum]
  enable_transport = ${ROE_TRANSPORT:-No}
  share_instance = Yes
  respond_to_probes = Yes

[logging]
  loglevel = 4

[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = ${ROE_DEVICE:-eth0}
    binary = /usr/local/bin/roe
EOF
fi

# No arguments, or only options: run rnsd with them.
if [ "$#" -eq 0 ] || [ "${1#-}" != "$1" ]; then
    exec rnsd --config "$CONFIG_DIR" "$@"
fi
exec "$@"
