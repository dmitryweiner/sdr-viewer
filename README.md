# SDR Viewer

A console app for the [ClockworkPi uConsole](https://www.clockworkpi.com/uconsole)
with the HackerGadgets AIO V2 board. It watches the air through the board's
RTL-SDR and shows what a person would otherwise miss: when, where and how
often something was transmitted.

**Status: phase 2 of 8 done** — the spectrum and waterfall screen works on
the built-in signal generator, on IQ recordings and on an RTL-SDR, with
tuning, gain and zoom from the keyboard; the other screens are still to
come. See
[plans/PLAN.md](plans/PLAN.md) for what comes next.

## Run

```sh
cargo run --release                          # the built-in generator
cargo run --release -- capture.cu8 --rate 2.4M --center 433.92M
cargo run --release -- --rtlsdr --center 433.92M --gain 30   # an RTL-SDR
cargo run --release -- --help
```

The RTL-SDR is driven by the pure-Rust `rtl-sdr-rs`; no `librtlsdr` is
needed. On Linux the kernel's DVB driver must not hold the device: blacklist
`dvb_usb_rtl28xxu` or unbind it.

Recordings: raw interleaved IQ as `rtl_sdr` writes it (`.cu8`, also `.raw`
and `.bin`), or `.cs8`, `.cs16`, `.cf32`; `--format` names it otherwise,
`--loop` plays it again and again.

Keys: `1`–`8` switch screens, `Tab` / `Shift+Tab` walk them, `?` help,
`q` quit. On the spectrum:

| Key | What |
|---|---|
| `←` `→` | move a frequency cursor (`Shift` — faster, `Esc` puts it away) |
| `Enter` | tune to the cursor |
| `f` | type a frequency: `433.92M`, `144800k`, `1.09G` |
| `[` `]` | tune down, up by half of what is visible |
| `+` `-` | zoom in, out (up to ×16) around the cursor |
| `g` `G` `a` | gain up, down a step of the tuner; automatic |
| `Space` | pause |
| `m` | reset max hold |

On the signal log (`5`) — one row per signal the detector found: when it
began, how long it lasted, where, how wide, how strong:

| Key | What |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select an event; the list holds still until `Esc` |
| `+` `-` | raise, lower the level above the noise below which events are hidden (10 dB at the start); hidden events are kept |
| `s` | sort by time, level, duration, frequency |
| `Enter` | tune to the selected event and show the spectrum |

Without a terminal — the screen as text, the strongest signals and the
detected events of the first seconds of a recording:

```sh
cargo run --release -- capture.cu8 --rate 2.4M --center 433.92M --snapshot 160x45 --secs 10
```

On the uConsole: take the static `aarch64` binary from the CI artifact
`sdr-viewer-aarch64`. The first run on a device is the
[hardware check](docs/hardware-check.md).

## Develop

```sh
./scripts/check.sh      # fmt + clippy + tests, the same as CI
```

## License

GPL-3.0-or-later, see [LICENSE](LICENSE).
