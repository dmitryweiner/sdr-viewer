# SDR Viewer

A console app for the [ClockworkPi uConsole](https://www.clockworkpi.com/uconsole)
with the HackerGadgets AIO V2 board. It watches the air through the board's
RTL-SDR and shows what a person would otherwise miss: when, where and how
often something was transmitted.

**Status: phase 3 of 8 done, phase 4 begun** — the spectrum and waterfall
screen, the signal log and the spectral density work on the built-in
signal generator, on IQ recordings and on an RTL-SDR: tuning, gain and
zoom from the keyboard, detected events kept in a database, their pictures
and their samples. The other screens are still to come. See
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

The status line at the bottom names the source and the rate samples
arrive at. Beside it, only when there is something to tell: `clip 2.4%` —
the share of samples at full scale, the receiver is overloaded (yellow
from 0.1%, red from 1%: lower the gain with `G`); `lost 0.05 s` — samples
the receiver delivered and the app did not take in time, since the start.

The spectral density (`3`) answers "which frequencies are busy at all?":
frequency runs across and level up, as on the spectrum, and a point is the
brighter, the more of the time the signal was at that level. The noise is
a bright band at the bottom, a carrier a bright spot above it, and a short
burst that an eye on the waterfall misses stays as a faint mark over its
frequency. Brightness grows with the logarithm of the share of time: from
one hundredth of a second in a hundred seconds to "always". The picture
forgets: what happened 30 s ago weighs half of what happens now. It is
gathered from the start, whichever screen is open.

The band, the zoom, the cursor and the scale are the spectrum's, and so
are its keys. With the cursor on, the header tells how much of the time
the frequency under it was above the detector's threshold (`busy 4.1%`).
`m` starts the picture afresh; `Space` holds it still.

On the signal log (`5`) — one row per signal the detector found: when it
began, how long it lasted, where, how wide, how strong:

| Key | What |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select an event; the list holds still until `Esc`, and the event's picture appears under it |
| `+` `-` | raise, lower the level above the noise below which events are hidden (10 dB at the start); hidden events are kept |
| `s` | sort by time, level, duration, frequency |
| `Enter` | tune to the selected event and show the spectrum |
| `r` | save the selected event's IQ samples |

The picture of the selected event is a small spectrogram made of its
samples: time runs across, from 50 ms before the event to 50 ms after it
(of a long one, the first 3 s), frequency runs up, four widths of the
event and at least 40 kHz; `▲` marks where the event begins and ends. It
needs the samples to be still held (see below), like saving.

### Saving an event's samples

The app holds the last 30 s of samples in memory (`--iq-buffer SECS`;
2 bytes a sample, 144 MB at 2.4 MS/s; `0` holds none), so an event can be
saved after it was seen: `r` on a selected row of the log writes its
samples, with 50 ms before and after, as a [SigMF](https://sigmf.org) pair
of files into `sdr-viewer/iq` under `$XDG_DATA_HOME` or `~/.local/share`
(`--iq-dir DIR` elsewhere):

```
20261112T170451.250Z_433.925MHz.sigmf-data   # cu8, the whole received band
20261112T170451.250Z_433.925MHz.sigmf-meta   # rate, frequency, time; the event as an annotation
```

The name is when the event began (UTC) and its centre; for a recording or
the generator, which do not run by the wall clock, the time in the stream
(`at-12.345s_…`). The data file is what `rtl_sdr` would have written, so
`rtl_433 -r`, `inspectrum` or `sdr-viewer FILE --format cu8` read it.

Only what is still held can be saved: an event older than the buffer, one
from before a retune, or one of an earlier run in the log's history has no
samples. A source finer than 8 bits (a `.cs16` or `.cf32` recording, the
generator) is saved in 8.

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

`--screen 3` prints the density screen instead of the spectrum.

On the uConsole: take the static `aarch64` binary from the CI artifact
`sdr-viewer-aarch64`. The first run on a device is the
[hardware check](docs/hardware-check.md).

## Develop

```sh
./scripts/check.sh      # fmt + clippy + tests, the same as CI
```

## License

GPL-3.0-or-later, see [LICENSE](LICENSE).
