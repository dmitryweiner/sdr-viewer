# Agents guide

SDR Viewer: a Rust TUI for the uConsole (CM4, aarch64) with the AIO V2
board. It watches the air through an RTL-SDR and shows what a person would
miss. The agreed plan is [plans/PLAN.md](plans/PLAN.md) — read it before
changing behaviour; agreed decisions go into its section 12. Docs, UI
strings and code comments are in English; the user talks to agents in
Russian. The shared conventions are in `~/projects/CLAUDE.md` (written for
web apps: TDD, scripts, docs, license and branches apply here too).

## Where things stand

Phases of the plan (its section 14 has the detail):

| Phase | State |
|---|---|
| 0 Hardware check | done — `docs/hardware.md` |
| 1 Skeleton | done |
| 2 Spectrum and waterfall | done on the generator, recordings and the RTL-SDR |
| 3 Detector and log | **built, pull requests #14–#18 open**: the noise floor, the detector, the log screen, the SQLite event database (7 days kept), the log's history with wall-clock time, the detector's threshold line on the spectrum, the IQ ring buffer, the picture of the selected event and "save IQ" in the log work; the detector's regression tests on a piece of real air (`testdata/lpd433.cu8`) pass; the status line tells overload and lost samples. Not yet tried on the air on the device |
| 4–8 | not started |

Nothing is left of phase 3's list. Before phase 4: merge #14–#18 in
order (they are stacked), and one run on the air on the device with the
user at it (the plan's section 14, "Known limits", says what has only
seen the recording).

## Commands

- `./scripts/check.sh` — `fmt --check` + `clippy -D warnings` + tests, the
  same as CI. Must pass before any work is called done.
- `cargo run --release [-- FILE --rate 2.4M --center 433.92M --loop]` — the
  app; without a file the demo generator runs.
- `cargo run --release -- --rtlsdr --center 433.92M [--gain 30] [--rate 2.4M]`
  — receive from an RTL-SDR. Events go to
  `~/.local/share/sdr-viewer/events.db` (`--db FILE` elsewhere, and for a
  recording or the generator too; `--no-db` nowhere). On the user's
  uConsole only through `sdr-viewer-rx.sh` (see "The user's device").
- With any source: `--iq-buffer SECS` (30; 0 = none) is the IQ ring in
  memory, `--iq-dir DIR` (default `~/.local/share/sdr-viewer/iq`) is where
  `r` in the log saves an event's samples as SigMF.
- `cargo run --release -- [FILE … | --rtlsdr …] --snapshot 160x45 [--secs 10]`
  — no terminal: analyses the start of the stream (unpaced for files and
  the generator), prints the spectrum screen as text, the max-hold
  peaks and the detector's events. The way to look at a recording, at the air, or at the picture from
  a script. Below 80x24 it prints only "too small".
- `cargo test --release -p sdr-core -- --ignored --nocapture bench_` — cost
  of the generator and of the IQ ring per sample.

CI (`.github/workflows/ci.yml`) runs on pull requests and on `main`:
`check.sh` on Linux x86-64; tests and a static `aarch64-unknown-linux-musl`
release build on a native arm64 runner, uploaded as the artifact
`sdr-viewer-aarch64` — the binary for the device. A branch without a pull
request gets no run: open a draft one to get a binary.

## Scripts

- `scripts/check.sh` — above.
- `scripts/smoke.py [--size 80x24] [--keys '12?'] [--secs 5] [-- ARGS]` —
  the release binary in a real pty: presses keys, quits with `q`, fails on
  a crash or non-zero exit, prints the CPU used and the bytes sent to the
  terminal. Enter is `\r` (`--keys $'f100M\r'`). `--bin PATH` runs a given
  binary; on the uConsole it needs only Python 3. `--words` prints the
  pieces of text the app wrote, to grep for what must be on the screen. An
  escape sequence in `--keys` is one key (`$'5\x1b[Br'`: log, down, save
  IQ). The default keys end
  on a placeholder screen: to measure a screen pass `--keys 1` (spectrum)
  or `--keys 5` (log), and compare builds side by side in one sitting.
- `scripts/term-check.sh` — can this terminal show truecolor, bars, half
  blocks, Braille.
- `scripts/hub-speed.sh` — for the user's device, installed there as
  `/usr/local/sbin/sdr-hub-speed` (root): `high [--while-pid PID]` puts the
  hub at 480M for the RTL-SDR and unbinds the kernel's DVB driver, `full`
  returns to 12M, `status`.
- `scripts/sdr-viewer-rx.sh ARGS` — on the device: `high` for as long as
  `sdr-viewer --rtlsdr ARGS` runs, then back, however the app exits.
- `scripts/hs-probe.sh` — the first 480M probe with `rtl_sdr`/`rtl_test`;
  kept as a cross-check against librtlsdr.

## The user's device

`ssh dmw@192.168.1.246` (key login; `sudo` works without a password).
Allowed without asking: read-only checks, our files in
`~/sdr-viewer-check/`, and installing `aiov2_ctl`, `rtl-sdr`,
`meshtasticd`, `gpsd`. Anything else that changes the system — ask first.
Everything found there is in `docs/hardware.md`; read its section 5 before
touching USB.

- **The hub is defective and deliberately held at 12 Mbit/s** by the user's
  udev rule; at 480M the built-in keyboard and trackball die, at 12M the
  RTL-SDR delivers nothing. Receive only through
  `~/sdr-viewer-check/sdr-viewer-rx.sh ARGS`; it switches the hub for the
  time the app runs (about 6 s each way). Tell the user before every run:
  their keyboard drops.
- Never run plain `rtl_*` tools or `sdr-viewer --rtlsdr` at 12M: they get
  no data and reset the hub.
- A new binary: download the PR's `sdr-viewer-aarch64` artifact
  (`gh run download RUN -n sdr-viewer-aarch64`) and `scp` it to
  `~/sdr-viewer-check/sdr-viewer`. What lies there now is PR #18's build
  (the threshold line on the spectrum, the IQ ring, the picture and `r` in
  the log, overload and lost samples in the status line), with PR #15's
  `smoke.py` (arrow keys). Nobody has looked at
  that build in the device's own terminal yet: how lxterminal's font draws
  the Braille threshold line and the picture is for the user's eye. Saving IQ was tried there on the recording with
  `--iq-dir iq-test`, removed afterwards; the default
  `~/.local/share/sdr-viewer/iq` does not exist yet.
- The default database `~/.local/share/sdr-viewer/events.db` does not
  exist on the device yet: since the database was added nobody has
  received from the RTL-SDR there, so that path is proven only by tests.
  Checks with a database used `--db test.db` in `~/sdr-viewer-check/` and
  removed it.
- `~/sdr-viewer-check/capture-433.cu8` — 10 s at 433.92 MHz, 2.4 MS/s,
  gain 30 dB, antenna on: LPD433 activity over a −71 dBFS floor. The
  detector finds 824 events in it: mostly 40–50 ms bursts ~9 kHz wide on
  the 25 kHz LPD grid (433.075 MHz + n × 25 kHz) over some 66 channels — a
  frequency hopper. `testdata/lpd433.cu8` in the repository is 0.16 s of
  it, from 4.96 s.
- Not set up yet: `meshtasticd` (LoRa is on `/dev/spidev4.0`), `gpsd`
  (installed, not configured), `aiov2_ctl`.
- The terminal there: lxterminal, truecolor; the user sets the font size.

## Plans and docs

| File | What |
|---|---|
| [plans/IDEA.md](plans/IDEA.md) | The original idea, as first written |
| [plans/PLAN.md](plans/PLAN.md) | The agreed plan: screens, architecture, phases, decisions (12), open questions (13), progress (14) |
| [docs/hardware.md](docs/hardware.md) | What phase 0 found on the device, and what is installed there |
| [docs/hardware-check.md](docs/hardware-check.md) | The phase 0 checklist, for another device |
| [testdata/README.md](testdata/README.md) | The recordings tests run on: what they are and how they were cut |

## Layout

A Cargo workspace under `crates/` (the plan's section 4 lists all eight;
a crate is added in the phase that needs it):

- `sdr-core` — `Source` trait (`read`, `tune`, `set_gain`,
  `lost_samples`) and `Iq` samples; `clip` (samples at full scale); `generator` (seeded tones, gated bursts, noise), `file` (raw IQ
  formats, looping), `pace` (real-time playback, `Clock` trait), `rate`
  (`RateMeter`), `units` (parse/format Hz), `frame` (`SpectrumFrame`: dBFS
  per bin, lowest frequency first), `ring` (`IqRing`: the newest samples
  as `cu8` pairs, numbered from the start of the stream).
- `sdr-dsp` — `analyzer`: Hann window, FFT, power averaged over a number
  of blocks into one `SpectrumFrame` (2048 bins); `average`:
  `FrameAverager`, fast frames into slow ones.
- `sdr-detect` — `floor` (`NoiseFloor`: per-segment medians followed over
  time, slow up and fast down) and `detector` (`Detector`: bins above the
  floor joined across frequency and time into `Event`s; keying splash of a
  strong signal masked; `Levels`: its floor per bin and its threshold,
  for the screen).
- `sdr-store` — `Store`: the SQLite event database (`rusqlite`, SQLite
  compiled in), `sessions` and `events`, the layout's version in
  `user_version`; `sigmf`: a recording as a `.sigmf-data` + `.sigmf-meta`
  pair (the JSON is written by hand).
- `sdr-rtl` — `RtlSource`: the RTL-SDR through `rtl-sdr-rs` (pure Rust on
  `nusb`), queued transfers, gain along the tuner's own steps, retuning.
- `sdr-tui` — `app` (state, keys and the `Command`s they ask of the
  source; no drawing), `ui` (frame of the interface, `render_text`),
  `spectrum` (the spectrum screen's state and arithmetic: columns, scale,
  zoom, palette), `spectrum_ui` (drawing it), `log` (the signal log's
  state: the last 10 000 events, the level filter, the order, a held
  list while a row is selected), `log_ui`, `fragment` (`Fragment`: the
  spectra around one event, and their reduction to pixels; `Picture`:
  waiting, gone or ready). Snapshot tests in
  `src/snapshots/`.
- `sdr-viewer` — the binary: `cli` (arguments → source), `pipeline`
  (samples → detector frames at 100 a second → events, and their averages
  → screen frames at 10 a second, each with the detector's `Levels`; one
  pass of FFTs), `reader` (the source
  thread: carry out `Command`s, read, fill the IQ ring, run the pipeline,
  send `Update`s, hand ended events to the store and an event's samples
  (`Cut`, as a `Job`) to the worker — its `Sinks`), `jobs` (the worker
  thread: saves a cut, or makes its picture; of pictures that piled up
  only the last), `save` (a `Cut` to a SigMF pair), `fragment` (a `Cut`
  to a `Fragment`), `keep` (open the database, read the
  week's history, start the writer thread, which first deletes what is
  older), `writer` (that thread's loop: one write a second),
  `snapshot` (`--snapshot`), `main` (terminal loop, redraws only when
  something changed).

## Invariants

- **TDD**: red → green → refactor; a bug fix starts with a failing test.
- Time, randomness and I/O are parameters (`Clock`, the generator's seed,
  `Read + Seek`), so tests are exact and never sleep.
- The interface never blocks reception: the reader thread `try_send`s its
  status and drops what the UI has no room for.
- Sources that are not hardware are wrapped in `Paced`; hardware paces
  itself.
- Overload is counted on the samples (a part at ±1.0, an 8-bit
  receiver's bytes 0 and 255), never guessed from the spectrum.
- Levels are dBFS: a full-scale tone on a bin reads 0. Power is averaged,
  never decibels. Frames run from the lowest frequency up, the centre is
  bin `len / 2`.
- The detector sees every frame; only the screen's frames may be dropped.
  Ended events are never dropped by `try_send`: the reader keeps them
  until the interface has room. The end of the stream waits for room too.
- Only the writer thread touches the database after start-up; the reader
  hands events over an unbounded channel and never waits for the disk. A
  change of the tables raises `SCHEMA_VERSION` and migrates older files;
  a file from a newer build is refused, never rewritten.
- The reader never waits for the disk when samples are saved, nor for
  the FFTs of a picture: it copies the samples out of the ring (`Cut`,
  for a picture 3 s at most) and the worker thread does the rest. The
  interface asks for a picture once, when the selection comes to a row,
  and drops one that arrives for a row it has left. Sample
  number N of the ring is stream time N / rate, the same count the
  analyser's frame times come from. A retune clears the ring.
- An event's wall-clock time is its session's start plus its stream time.
  The log shows wall-clock time only when there is a database (`WallClock`
  with the local UTC offset, taken once at the start through `chrono` —
  the only place the time zone is asked for).
- The main loop does not redraw for events alone: they show with the next
  frame or status (10 a second).
  An event's times are stream time (`SpectrumFrame::time`), not the clock.
- The palette's brightness only grows with the level (a test holds it).
- The spectrum has two floors on purpose: the scale follows the frame's
  median (fast), the threshold line and "above floor" use the detector's
  floor, which comes with every frame. A bar always wins its cell over the
  line.
- The interface never touches the source: keys become `Command`s, the
  reader thread carries them out and answers with `Update::Source` or
  `Update::Notice`.
- `unsafe_code = "forbid"` across the workspace.
- The numbers of the reference recording's tests (`pipeline.rs`) change
  only on purpose too: look at what the detector now finds in
  `testdata/lpd433.cu8` first.
- `ui` snapshots change only on purpose: review `cargo insta review` (or
  `INSTA_UPDATE=always cargo test` and read the diff) before committing.
- Each feature gets its own branch and a pull request.

## What costs what

Measured on the dev Mac (Apple Silicon), release build:

| What | Cost |
|---|---|
| `./scripts/check.sh` after touching `sdr-core` | ~2 s |
| Generator, demo scene (2 tones + noise) | ~16 ns/sample = ~4% of a P-core at 2.4 MS/s; noise is 1.7, tones 10.5 |
| App with the generator, spectrum screen at 160×45 (`smoke.py --keys 1`) | 7–15% of one core depending on the day; the detector adds nothing measurable (three builds side by side: 13, 13, 13%), nor does the threshold line (12 and 15% before, 15 and 15% with it). ~0.8 MB/s to the terminal |
| Live reception on the CM4, RTL-SDR at 2.4 MS/s, spectrum at 160×45 | **31%** of one core (0.6 s of 15 in the kernel); 30 s of samples arrive in 30.0 s; opening the device takes 1.15 s |
| On the CM4, three builds side by side, 160×45, 15 s each (`smoke.py --keys 1` spectrum / `--keys 5`) | playing `capture-433.cu8`: spectrum **30%** before the detector, **32%** with it, **33%** with the log; screen 5 **15%** (placeholder), **18%** (placeholder + detector), **23%** (the log, ~80 events a second). Generator: spectrum 37 / 38 / 28% (the last looks like an outlier), screen 5 22 / 25 / 24% |
| What that splits into on the CM4 | the detector ~3 points; the spectrum screen ~15 (0.9 MB/s to the terminal); the log on a busy band ~5. The terminal emulator's own cost is not measured (the smoke has no window) |
| Generator alone in the app, before the spectrum (phase 1) | 6%; **8% on the CM4** |
| App playing a `.cu8` file | ~2% of one core |
| The event database on the CM4 (`smoke.py --keys 5`, the recording, with and without `--db`) | nothing measurable: 22% against 21–22% of one core. The file grows ~93 bytes an event: on the recording's band (~80 events a second) that is ~660 MB a day of reception |
| The IQ ring buffer (30 s; with and without `--iq-buffer 0`) | `bench_ring` on the Mac: 0.3 ns/sample; 153 MB resident after 40 s, as computed (144 MB + the app). **On the CM4**, the recording, 15 s: spectrum (`--keys 1`) 32 → 34% of a core, log (`--keys 5`) 21 → 22%. With a bounds check and a wrap test per sample it was 4 points, not 1–2 |
| The picture of the selected event on the CM4 (the recording, 15 s, `smoke.py --keys 5` plus down arrows) | the log alone **22%** of a core; a row held with its picture **19%** (the list stands still); a new picture every 0.36 s (40 downs) **26%** |
| Counting clipped samples on the CM4 (the recording, 15 s, before and after) | within a point: spectrum 34 → 35% of a core, log 22 → 23% |
| Static aarch64 binary from CI | 4.6 MB with SQLite compiled in (2.4 before) |

Before phasors and the noise table the generator took 70% of a CM4 core:
measure on the device, the Mac's numbers do not scale.

## Pitfalls that already cost time

- **A reset that looks like a fault may be a workaround.** The hub's fall
  to 12M was first taken for a hardware failure; it was the user's own
  udev rule. Look in `/etc/udev/rules.d` and ask before diagnosing.
- **A background SSH command keeps running after its first step is
  killed.** A `for` loop over sample rates went on for ten minutes and
  reset the hub again; give every device command its own `timeout -s KILL`.
- **`gh run download` without a run id takes the latest run that has the
  artifact** — another branch's binary. Always pass the id.
- **One synchronous USB transfer at a time loses ~10% of the samples** at
  2.4 MS/s on the CM4; check that N seconds of samples arrive in N seconds.
- **The scale must fit the receiver**: with 70 dB the floor of an 8-bit
  receiver glowed and real signals drowned; a photo of the device showed
  it, the snapshot tests did not.
- **`cargo fmt` reflows code**, so a scripted edit anchored on the text as
  written stops matching: assert the anchor occurs exactly once.
- **A CPU number without its screen is worthless.** The detector's cost
  was first "measured" with the smoke's default keys, which leave the app
  on a placeholder, and compared with numbers taken on the spectrum. Name
  the keys with every number, and re-measure the old build alongside.
- **zsh**: `"$H:path"` applies a modifier to `$H`; write `"${H}:path"`.
