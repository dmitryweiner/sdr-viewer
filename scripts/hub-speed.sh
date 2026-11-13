#!/bin/sh
# For the user's uConsole with the defective hub (docs/hardware.md, section
# 5). The hub must run at 12 Mbit/s for the built-in keyboard and trackball
# and at 480 for the RTL-SDR; this switches between the two. Runs on the
# device, as root; installed there as /usr/local/sbin/sdr-hub-speed.
#
#   sudo sdr-hub-speed high [--while-pid PID]   # 480M, keyboard dead
#   sudo sdr-hub-speed full                     # 12M, the normal state
#   sdr-hub-speed status
#
# `high` also switches the RTL-SDR's power on (GPIO 7 of the AIO V2 board:
# after a boot nothing drives it, and the receiver is not on the bus at
# all), waits for the receiver to appear, and unbinds the kernel's DVB
# driver from it, which our driver cannot do itself. The power stays on. With --while-pid a watcher switches back to
# `full` when that process exits, however it exits. The switch is a flag
# file in /run that the udev rule of the workaround checks
# (TEST!="/run/sdr-viewer-high-speed"), so a reboot always returns to 12M.
# Prints the hub's speed at the end; exits 1 if the switch did not happen.
set -u
FLAG=/run/sdr-viewer-high-speed
DRV=/sys/bus/platform/drivers/dwc2
CTRL=fe980000.usb
HUB=/sys/bus/usb/devices/1-1
DVB=/sys/bus/usb/drivers/dvb_usb_rtl28xxu
SDR_POWER_PIN=7

speed() { cat $HUB/speed 2>/dev/null || echo none; }

rebind() {
  echo $CTRL > $DRV/unbind
  sleep 2
  echo $CTRL > $DRV/bind
}

# Waits until the hub reports the wanted speed and has stayed there.
settle() {
  i=0
  while [ $i -lt 40 ]; do
    sleep 0.5
    if [ "$(speed)" = "$1" ]; then
      sleep 2
      [ "$(speed)" = "$1" ] && return 0
    fi
    i=$((i + 1))
  done
  return 1
}

# True once a 0bda:2838 is on the bus; gives it 10 s.
rtl_present() {
  i=0
  while [ $i -lt 20 ]; do
    for dev in /sys/bus/usb/devices/*; do
      [ "$(cat "$dev/idVendor" 2>/dev/null)" = 0bda ] &&
        [ "$(cat "$dev/idProduct" 2>/dev/null)" = 2838 ] && return 0
    done
    sleep 0.5
    i=$((i + 1))
  done
  return 1
}

unbind_dvb() {
  i=0
  while [ $i -lt 20 ]; do
    for dev in "$DVB"/*:*; do
      [ -e "$dev" ] || continue
      echo "$(basename "$dev")" > $DVB/unbind 2>/dev/null
      return 0
    done
    sleep 0.5
    i=$((i + 1))
  done
  return 0
}

case "${1:-}" in
  status) ;;
  high)
    pinctrl set $SDR_POWER_PIN op dh
    touch $FLAG
    [ "$(speed)" = 480 ] || rebind
    if ! settle 480; then
      rm -f $FLAG; rebind; settle 12
      echo "the hub did not come up at 480M; now $(speed)M" >&2
      exit 1
    fi
    if ! rtl_present; then
      rm -f $FLAG; rebind; settle 12
      echo "no RTL-SDR (0bda:2838) on the bus with GPIO $SDR_POWER_PIN high; the hub is back at $(speed)M" >&2
      exit 1
    fi
    unbind_dvb
    if [ "${2:-}" = --while-pid ] && [ -n "${3:-}" ]; then
      ( while kill -0 "$3" 2>/dev/null; do sleep 1; done; "$0" full >/dev/null 2>&1 ) &
    fi
    ;;
  full)
    rm -f $FLAG
    [ "$(speed)" = 12 ] || rebind
    settle 12 || { echo "the hub did not return to 12M; now $(speed)M" >&2; exit 1; }
    ;;
  *)
    echo "usage: $0 high [--while-pid PID] | full | status" >&2
    exit 2
    ;;
esac
echo "hub $(speed)M"
