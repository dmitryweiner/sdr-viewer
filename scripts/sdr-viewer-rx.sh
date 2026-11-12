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
# this script, then in PATH.
set -e
HERE=$(dirname "$0")
BIN=$HERE/sdr-viewer
[ -x "$BIN" ] || BIN=sdr-viewer
sudo -n /usr/local/sbin/sdr-hub-speed high --while-pid $$ >&2
exec "$BIN" --rtlsdr "$@"
