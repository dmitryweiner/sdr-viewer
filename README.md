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

The dotted line over the noise is the detector's threshold, 8 dB above the
noise floor it follows: a bar that crosses it becomes an event in the log.
The header says how far above that floor the peak, or the cursor, is.

On the signal log (`5`) — one row per signal the detector found: when it
began, how long it lasted, where, how wide, how strong:

| Key | What |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select an event; the list holds still until `Esc` |
| `+` `-` | raise, lower the level above the noise below which events are hidden (10 dB at the start); hidden events are kept |
| `s` | sort by time, level, duration, frequency |
| `Enter` | tune to the selected event and show the spectrum |

### The event database

With `--rtlsdr` every detected event is also kept in a SQLite file:
`sdr-viewer/events.db` under `$XDG_DATA_HOME`, or under `~/.local/share`.
`--db FILE` names another file — and keeps the events of a recording or of
the generator too, which are not kept otherwise; `--no-db` keeps nothing.
Events are written once a second; those still on when the app quits are
ended there and written. Events older than 7 days are deleted when the app
starts.

With a database the log shows the local date and time of day instead of
the time in the stream, and starts with what earlier runs found in the
last 7 days (up to the 10 000 newest).

```sh
sqlite3 ~/.local/share/sdr-viewer/events.db "
  SELECT datetime(start_unix_ms / 1000, 'unixepoch', 'localtime'),
         duration_us / 1000 AS ms, round(peak_hz / 1e6, 3) AS mhz,
         round(above_floor_db, 1) AS db
  FROM events ORDER BY start_unix_ms DESC LIMIT 20"
```

`sessions` has one row per run (when it began, the source, its frequency
and rate); `events` has the session, the wall-clock start (the session's
start plus the time in the stream), the duration, the band, the peak and
its level above the noise floor.

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
