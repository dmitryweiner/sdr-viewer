#!/bin/sh
# For the user's uConsole with the defective hub (docs/hardware.md, section
# 5): lets the hub run at 480M, identifies the tuner, records 10 s at
# 433.92 MHz and counts lost samples, then restores the full-speed
# workaround whatever happens (trap on EXIT). Runs on the device, as root;
# the built-in keyboard and trackball are dead for about a minute.
#
#   scp scripts/hs-probe.sh dmw@192.168.1.246:sdr-viewer-check/
#   ssh dmw@192.168.1.246 'cd sdr-viewer-check && sudo nohup ./hs-probe.sh > hs-probe.log 2>&1; cat hs-probe.log'
#
# Prints the USB tree at high speed, the tuner, bytes recorded and wall
# time, lost samples, and the tree after restoring (the hub must be at 12M
# and the keyboard must name itself "uConsole"). Leaves capture-433.cu8.
RULE=/etc/udev/rules.d/99-dwc2-force-fs.rules
DRV=/sys/bus/platform/drivers/dwc2
OUT=/home/dmw/sdr-viewer-check
rebind() { echo fe980000.usb > $DRV/unbind; sleep 3; echo fe980000.usb > $DRV/bind; sleep 8; }
restore() {
  echo "--- restoring"
  [ -f $RULE.off ] && mv $RULE.off $RULE
  udevadm control --reload
  rebind; sleep 6
  lsusb -t | head -3
  cat /sys/bus/usb/devices/1-1.1/product 2>/dev/null
}
trap restore EXIT
mv $RULE $RULE.off && udevadm control --reload
rebind
echo "--- at high speed"; lsusb -t; cat /sys/bus/usb/devices/1-1/speed
echo "--- tuner"; timeout -s KILL 15 rtl_test -t 2>&1 | grep -iE "tuner|Found|locked"
echo "--- 10 s at 2.4 MS/s, 433.92 MHz"
S=$(date +%s.%N)
timeout -s KILL 30 sudo -u dmw rtl_sdr -f 433.92M -s 2400000 -g 30 -n 24000000 $OUT/capture-433.cu8 2>&1 | grep -iE "lost|error|fail|short|locked"
E=$(date +%s.%N); echo "bytes $(stat -c %s $OUT/capture-433.cu8) wall $(echo "$E - $S" | bc)"
echo "--- lost samples over 8 s"; timeout -s INT 8 rtl_test -s 2400000 2>&1 | grep -iE "lost|ppm" | tail -4
