# SDR Viewer

A console app for the [ClockworkPi uConsole](https://www.clockworkpi.com/uconsole)
with the HackerGadgets AIO V2 board. It watches the air through the board's
RTL-SDR and shows what a person would otherwise miss: when, where and how
often something was transmitted.

**Status: phase 2 of 8** — the spectrum and waterfall screen works on the
built-in signal generator and on IQ recordings; the RTL-SDR source and the
other screens are still to come. See
[plans/PLAN.md](plans/PLAN.md) for what comes next.

## Run

```sh
cargo run --release                          # the built-in generator
cargo run --release -- capture.cu8 --rate 2.4M --center 433.92M
cargo run --release -- --help
```

Recordings: raw interleaved IQ as `rtl_sdr` writes it (`.cu8`, also `.raw`
and `.bin`), or `.cs8`, `.cs16`, `.cf32`; `--format` names it otherwise,
`--loop` plays it again and again.

Keys: `1`–`8` switch screens, `Tab` / `Shift+Tab` walk them, `?` help,
`q` quit. On the spectrum: `←` `→` move a frequency cursor (`Shift` —
faster, `Esc` puts it away), `Space` pauses, `m` resets max hold.

Without a terminal — the screen as text and the strongest signals of the
first seconds of a recording:

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
