#!/bin/sh
# Receives from the RTL-SDR on the user's uConsole: switches the hub to
# 480M for as long as sdr-viewer runs (the built-in keyboard and trackball
# are dead meanwhile — use SSH or a Bluetooth keyboard), then back to 12M.
# Needs scripts/hub-speed.sh installed as /usr/local/sbin/sdr-hub-speed.
#
#   ./sdr-viewer-rx.sh --center 433.92M --gain 30
#   ./sdr-viewer-rx.sh --center 433.92M --snapshot 160x45 --secs 10
#
# Arguments go to `sdr-viewer --rtlsdr`. The binary is looked for next to
# this script, then in PATH. SDR and LoRa are never used together (the
# plan's section 12): a running meshtasticd is stopped for the time of
# reception and started again when the app exits, however it exits.
set -e
HERE=$(dirname "$0")
BIN=$HERE/sdr-viewer
[ -x "$BIN" ] || BIN=sdr-viewer
if systemctl -q is-active meshtasticd; then
  sudo -n systemctl stop meshtasticd
  # $$ is the app's pid after the exec below.
  (
    while kill -0 $$ 2>/dev/null; do sleep 1; done
    sudo -n systemctl start meshtasticd
  ) >/dev/null 2>&1 &
fi
sudo -n /usr/local/sbin/sdr-hub-speed high --while-pid $$ >&2
exec "$BIN" --rtlsdr "$@"
