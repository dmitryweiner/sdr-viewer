# Hardware check (phase 0)

A checklist to run on the uConsole once, before the RTL-SDR source is
written (plan, section 8, phase 0). It confirms the facts in the plan's
section 2 that came from the forum, and prices the CPU. Write the answers
into `docs/hardware.md` (create it) — the plan's risks in section 11 are
decided from them.

Copy to the device: the `sdr-viewer` binary from the CI artifact
`sdr-viewer-aarch64` (any successful run of the `aarch64` job), plus
`scripts/term-check.sh` and `scripts/smoke.py`.

## 1. System

```sh
cat /proc/device-tree/model; echo
uname -m; nproc; free -h
. /etc/os-release; echo "$PRETTY_NAME"
ldd --version | head -1
```

Expect a CM4, aarch64, 4 cores, 4 GB. The binary is static (musl), so the
glibc version only matters for tools we may build on the device.

## 2. Terminal

In the terminal the app will really run in (the default one of the image),
full screen:

```sh
./term-check.sh
```

- Is the gradient smooth (truecolor) or banded?
- Does any glyph show as a box or `?`: bars, half blocks, Braille?
- Columns × rows at the default font; is the font readable at that size?

## 3. The app and its cost

```sh
chmod +x sdr-viewer
./sdr-viewer                     # keys 1–8, Tab, ?, q — does everything draw?
python3 smoke.py --bin ./sdr-viewer --keys '' --secs 20
```

The second command prints the CPU of the demo generator at 2.4 MS/s plus
the interface; on a 2026 Mac it is about 25–30% of one core. Note the
number and the `top` view of the terminal emulator while the app runs.

## 4. AIO V2 power

```sh
aiov2_ctl --help                 # the board's tool; the exact name may differ
aiov2_ctl status                 # what is on: GPS, LoRa, SDR, USB
```

Find out how to switch the SDR (GPIO 7) and internal USB (GPIO 23) on, and
whether it needs root. If the tool is missing, note where it comes from.

## 5. RTL-SDR

With the SDR powered:

```sh
lsusb | grep -iE 'rtl|0bda'      # expect 0bda:2838 or similar
lsmod | grep -E 'dvb_usb_rtl28xxu|rtl2832'
ls -l /dev/bus/usb/*/*           # who may open it without root
rtl_test -t                      # package rtl-sdr: the tuner name (R860? R820T2?)
```

If `dvb_usb_rtl28xxu` is loaded, the kernel holds the device; note whether
the image already blacklists it (`grep -r rtl28xxu /etc/modprobe.d`).

A reference recording for the detector tests, 10 s at 433.92 MHz (press a
car key or a doorbell remote meanwhile, if one is at hand):

```sh
rtl_sdr -f 433.92M -s 2.4M -n 24000000 capture-433.cu8
./sdr-viewer capture-433.cu8 --rate 2.4M --center 433.92M
```

## 6. LoRa and Meshtastic

```sh
ls /dev/spidev*
systemctl status meshtasticd --no-pager
ss -ltn | grep 4403              # the TCP API the app will use
```

Note the LoRa region and frequency `meshtasticd` is set to (open question 1
in the plan).

## 7. GPS and clock

```sh
systemctl status gpsd --no-pager
ls /dev/ttyS* /dev/serial* 2>/dev/null
timedatectl
```
