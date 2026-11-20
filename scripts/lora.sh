#!/bin/sh
# For the user's uConsole with the HackerGadgets AIO V2 board
# (docs/hardware.md, section 6): switches the LoRa radio and meshtasticd
# on and off. Runs on the device, as root for on and off.
#
#   sudo ./lora.sh on       # power, SPI1, meshtasticd; waits for port 4403
#   sudo ./lora.sh off      # meshtasticd stopped, LoRa power off
#   ./lora.sh status
#
# `on` powers the SX1262 (GPIO 16: after a boot nothing drives it), loads
# the SPI1 overlay at run time when /dev/spidev1.0 is missing (it is not
# in config.txt, so a reboot drops it), and (re)starts meshtasticd, which
# must start after the power: it looks for the radio once. The service is
# not started at boot. Started without the radio, meshtasticd crashes and
# systemd gives up after a few tries ("start-limit-hit"); `on` clears
# that first. Prints the state at the end; exits 1 if SPI1 did not come
# or the API port did not open. Without root it changes nothing and says
# so (the power pin alone would switch: the gpio group may).
set -u
LORA_POWER_PIN=16
SPIDEV=/dev/spidev1.0
PORT=4403

listening() { ss -ltn 2>/dev/null | grep -q ":$PORT "; }

status() {
  echo "power: $(pinctrl get $LORA_POWER_PIN | sed 's/.*| \([a-z-]*\).*/\1/')"
  [ -e $SPIDEV ] && echo "spi: $SPIDEV" || echo "spi: none"
  echo "meshtasticd: $(systemctl is-active meshtasticd)"
  listening && echo "api: port $PORT" || echo "api: closed"
}

as_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "$0 $1: needs root — sudo $0 $1" >&2
    exit 1
  fi
}

case "${1:-}" in
on)
  as_root on
  pinctrl set $LORA_POWER_PIN op dh
  [ -e $SPIDEV ] || dtoverlay spi1-1cs
  i=0
  while [ $i -lt 10 ] && [ ! -e $SPIDEV ]; do
    sleep 0.2
    i=$((i + 1))
  done
  if [ ! -e $SPIDEV ]; then
    echo "no $SPIDEV after dtoverlay spi1-1cs; meshtasticd not started" >&2
    status
    exit 1
  fi
  sleep 0.5
  systemctl reset-failed meshtasticd 2>/dev/null
  systemctl restart meshtasticd
  i=0
  while [ $i -lt 30 ] && ! listening; do
    sleep 0.5
    i=$((i + 1))
  done
  status
  listening
  ;;
off)
  as_root off
  systemctl stop meshtasticd
  pinctrl set $LORA_POWER_PIN op dl
  status
  ;;
status)
  status
  ;;
*)
  echo "usage: $0 on|off|status" >&2
  exit 2
  ;;
esac
