# Hardware notes (phase 0)

What was found on the user's uConsole on 2026-11-12, following
[hardware-check.md](hardware-check.md). Read-only checks over SSH; nothing
was installed or changed on the device apart from `~/sdr-viewer-check/`
(our binary and two scripts).

Status: **blocked on USB** (section 5): the RTL-SDR delivers no samples
because the internal hub falls back to 12 Mbit/s after boot. Meshtastic and
GPS (6, 7) are not set up yet.

Installed on the device since, with the user's permission: `rtl-sdr`
2.0.2, `gpsd` 3.25, `gpsd-clients` (Debian packages). `sudo` works without
a password there.

## 1. System

| | |
|---|---|
| Module | Raspberry Pi Compute Module 4 Rev 1.0, 4 × Cortex-A72, 600–1500 MHz |
| RAM | 8 GB (the plan assumed 4) |
| OS | Debian 13 (trixie), kernel 6.12.62-v8+, aarch64, glibc 2.41 |
| Desktop | labwc (Wayland) + Xwayland |
| Terminal | lxterminal 0.4.1 on VTE 0.80 (VTE supports truecolor) |
| Disk | 29 GB, 16 GB free |
| User | in `plugdev`, `gpio`, `spi`, `i2c`, `dialout` |

Not installed: `rtl-sdr` tools, `rtl_433`, `meshtasticd`, `gpsd`,
`aiov2_ctl`, Rust. Python 3, `pinctrl` and `gpioget` are there.

## 2. Terminal

`scripts/term-check.sh` in lxterminal on the device's screen (the user's
photo, 2026-11-12): `COLORTERM=truecolor`, the gradient is smooth, bars,
half blocks, Braille and box lines all draw.

**The default font gives 79 × 20 cells**, below the app's 80 × 24 minimum,
so out of the box the app only says "too small". The plan's 160 × 45
needs an 8 × 16 px font. Open: ship a font setting / launcher with the
app, or make the layout work from about 79 × 20.

## 3. Our binary and its cost

The static musl binary from CI (`sdr-viewer-aarch64`) runs as is.
`smoke.py --keys '' --secs 20`: the app with the demo generator at
2.4 MS/s, 160×45, idle.

| Generator | CM4, one core | Dev Mac |
|---|---|---|
| `sin_cos` per tone, Box–Muller per sample (phase 1) | **70%** (~290 ns/sample) | 25–30% |
| Phasors and a noise table | **8%** | 6% |

The terminal's own cost was not measured: the smoke runs in a pty without
a window.

## 4. AIO V2 power

`aiov2_ctl` is not installed. The pins as found (`pinctrl get`):

| GPIO | Module (plan) | State |
|---|---|---|
| 7 | SDR | output, high |
| 16 | LoRa | output, high |
| 27 | GPS | output, high |
| 23 | internal USB | **input, pulled down, low** |

SDR, LoRa and GPS look powered, and the RTL-SDR is on the bus. What set
the pins (boot overlay or a script) is not known yet, nor why GPIO 23 is an
input while USB works; the user is in the `gpio` group, so the app could
switch them without root.

## 5. RTL-SDR

- On the bus: `0bda:2838`, strings `HackerGadgets` / `AIO_V2 Ext`, serial
  25120901. Tuner: **Rafael Micro R820T** (`rtl_test -t`), 29 gain steps,
  0–49.6 dB.
- The user may open it (ACL `user:dmw:rw-`); no udev rule of ours needed.
- The kernel's `dvb_usb_rtl28xxu` is bound to it and nothing blacklists it,
  but `librtlsdr` detaches the driver itself, without root, and reattaches
  it on exit. Our source must do the same.
- Whether `rtl-sdr-rs` works with it: not checked yet.

### The hub falls back to 12 Mbit/s — no samples arrive

`rtl_sdr -f 433.92M -s 2400000 -n 12000000` wrote **0 bytes** in two
minutes and ignored SIGTERM; it had to be killed. During the attempt the
whole hub disconnected and came back twice (the keyboard is on the same
hub and drops with it).

`lsusb -t`: the root port is 480M; the Genesys hub `05e3:0608` under it,
and everything behind it, run at 12M. 2.4 MS/s of 8-bit IQ is 38 Mbit/s.

`dmesg` shows it is not so from power-on:

| Time | Event |
|---|---|
| 3.0 s | hub `1-1` enumerates **high-speed**; the RTL-SDR and the second hub too |
| 7.3–8.2 s | the RTL-SDR, the keyboard (still its bootloader `1eaf:0003`) and the second hub disconnect |
| 13.2 s | hub `1-1` disconnects; `usb1-port1: attempt power cycle` |
| 16.2 s | hub `1-1` is back at **full speed**: "not running at top speed" |
| 145.8 s | the RTL-SDR appears (full speed) |

It is not a boot script. Rebinding the controller on a running system
(`echo fe980000.usb > /sys/bus/platform/drivers/dwc2/unbind`, then `bind`)
repeats it every time: the hub enumerates high-speed, **no device behind it
appears for 5 s**, the hub disconnects, the port is power-cycled, and the
hub returns at full speed with everything behind it. The same happens with
the SDR's power off (GPIO 7 low), so the RTL-SDR is not the cause: the
high-speed link to the hub `05e3:0608` itself does not hold.

At full speed no sample rate works: `rtl_sdr` hung with 0 bytes at 2.4 and
1.024 MS/s, and returned 0 bytes at 250 kS/s. No under-voltage
(`vcgencmd get_throttled` = `0x0`). USB is set up by
`dtoverlay=dwc2,dr_mode=host` in `config.txt` (image 2025-12-30).

Not known: whether the cause is mechanical (this is the board's first
power-up in this device — seating of the AIO board and its connector) or
the `dwc2` driver, and whether the hub held high speed before the AIO
board was fitted.

## 6. LoRa

`/dev/spidev4.0` exists, **not** `spidev1.0` as the plan says
(`dtparam=spi=on`, `dtoverlay=spi0-0cs` in `config.txt`). `meshtasticd` is
not installed, nothing listens on 4403.

## 7. GPS and clock

`gpsd` is installed now but not configured. `/dev/ttyS0` exists, and
`serial-getty@ttyS0` and `ModemManager` are running — either may hold the
GPS UART. Not checked further.
