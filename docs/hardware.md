# Hardware notes (phase 0)

What was found on the user's uConsole on 2026-11-12, following
[hardware-check.md](hardware-check.md). Read-only checks over SSH; nothing
was installed or changed on the device apart from `~/sdr-viewer-check/`
(our binary and two scripts).

Status: the RTL-SDR works only while the hub runs at 480M, which kills
the built-in keyboard (section 5); `rtl-sdr-rs` is not tried yet. Meshtastic and
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

### The hub is forced to 12 Mbit/s on purpose — and the RTL-SDR needs 480

This device's mainboard hub (Genesys `05e3:0608`) has a defect: bit 6 of
every byte passing through its Transaction Translator reads as 0, which
corrupts all full/low-speed traffic (keyboard, trackball) whenever the
hub's uplink runs at high speed. The user's workaround
([gist](https://gist.github.com/dmitryweiner/1023d39419e7ebb90be3388e3d979f67)):
`/etc/udev/rules.d/99-dwc2-force-fs.rules` runs
`/usr/local/sbin/dwc2-force-fs.py`, which sets `HCFG.FSLSSUPP` in the dwc2
controller and resets the port whenever the hub appears at 480M. That is
the "disconnect, power cycle, back at full speed" seen in `dmesg` at boot
and after every rebind. The proper fix is a mainboard replacement.

So on this device, until the mainboard is replaced:

| Hub uplink | Keyboard, trackball | RTL-SDR |
|---|---|---|
| 12M (the workaround, the normal state) | work | **no samples at all** |
| 480M | garbage descriptors, dead | **works** |

Measured at 12M: `rtl_sdr` wrote 0 bytes at 2.4 MS/s and 1.024 MS/s (hung,
ignored SIGTERM) and at 300 and 250 kS/s (12 s each). The RTL2832U
enumerates at full speed (bulk IN `0x81`, 64-byte packets) but does not
stream; every attempt also made the hub re-enumerate, which drops the
keyboard for a second. Do not run RTL-SDR tools in this state.

Measured at 480M (`scripts/hs-probe.sh`: the udev rule set aside, the
controller rebound, everything restored on exit): the RTL-SDR enumerates
high-speed; `rtl_sdr` at 2.4 MS/s wrote 48 000 000 bytes in 10.9 s
including start-up; `rtl_test` over 8 s lost 5 samples per million. The
recording (433.92 MHz, gain 30 dB) is noise at about −39 dBFS, bytes
113–142, no clipping; the antenna was connected. Its spectrum (`--snapshot`) shows
real air: a noise floor of −71 dBFS per bin and about ten signals 22–28 dB
above it in max hold, several on the 25 kHz grid of the LPD433 channels
(434.153, 434.178, 434.228, 434.253 MHz) — so the tuner is where it was
asked to be. The
tuner prints `[R82XX] PLL not locked!` on opening — common with this tuner
and harmless: the data flows and the tuning is right (above). Our app plays that recording on the CM4 at 5% of one core.

Agreed with the user (2026-11-12): until the mainboard is replaced the app
gets a **"reception without the keyboard" mode** — it lets the hub run at
480M while receiving and restores 12M afterwards; control over SSH or a
Bluetooth keyboard meanwhile.

## 6. LoRa

`/dev/spidev4.0` exists, **not** `spidev1.0` as the plan says
(`dtparam=spi=on`, `dtoverlay=spi0-0cs` in `config.txt`). `meshtasticd` is
not installed, nothing listens on 4403.

## 7. GPS and clock

`gpsd` is installed now but not configured. `/dev/ttyS0` exists, and
`serial-getty@ttyS0` and `ModemManager` are running — either may hold the
GPS UART. Not checked further.
