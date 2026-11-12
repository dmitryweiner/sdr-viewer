# Hardware notes (phase 0)

What was found on the user's uConsole on 2026-11-12, following
[hardware-check.md](hardware-check.md). Read-only checks over SSH; nothing
was installed or changed on the device apart from `~/sdr-viewer-check/`
(our binary and two scripts).

Status: **partly done**. Still to do, in order: the terminal check on the
device's own screen (2), a real IQ stream from the RTL-SDR (5), Meshtastic
and GPS (6, 7) — nothing for them is installed yet.

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

## 3. Our binary and its cost

The static musl binary from CI (`sdr-viewer-aarch64`) runs as is.

| What | Cost on the CM4 |
|---|---|
| App with the demo generator at 2.4 MS/s, 160×45, idle (`smoke.py --keys '' --secs 20`) | **70% of one core** (13.9 s user in 20 s ≈ 290 ns/sample) |
| The same on the dev Mac | ~25–30% of an E-core, ~40 ns/sample on a P-core |

So the generator is ~7× dearer here than on the Mac and leaves little of
its core. It is a development source, not what runs on the device, but
phase 2 tests on the device will use it: make it cheaper first (phasor
rotation instead of `sin_cos`, a noise table instead of Box–Muller per
sample), or run it at a lower rate there. The terminal's own cost was not
measured: the smoke runs in a pty without a window.

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

- On the bus: `0bda:2838 Realtek RTL2838 DVB-T`; the user may open it
  (ACL `user:dmw:rw-`), no udev rule of ours is needed.
- **The kernel holds it**: `dvb_usb_rtl28xxu` is bound to interface 0
  (with `rtl2832`, `rtl2832_sdr`), and nothing in `/etc/modprobe.d`
  blacklists it. The app must detach the kernel driver when opening, or
  the package must ship the blacklist (plan, section 10).
- **The internal hub runs at 12 Mbit/s.** `lsusb -t`: the root port is
  480M, but the Genesys hub `05e3:0608` under it, and the RTL-SDR behind
  it, are at 12M (full speed). 2.4 MS/s of 8-bit IQ is 38 Mbit/s and does
  not fit into 12. To be confirmed with a real stream — if it holds, the
  usable sample rate on this device is far below the plan's 2.4 MS/s, and
  the reason (hub, cable, `dwc2` settings) has to be found first.
- Tuner (R860 or R820T2) and whether `rtl-sdr-rs` works with it: not
  checked yet, needs a tool on the device.

## 6. LoRa

`/dev/spidev4.0` exists, **not** `spidev1.0` as the plan says
(`dtparam=spi=on`, `dtoverlay=spi0-0cs` in `config.txt`). `meshtasticd` is
not installed, nothing listens on 4403.

## 7. GPS and clock

`gpsd` is not installed; `/dev/ttyS0` exists. Not checked further.
