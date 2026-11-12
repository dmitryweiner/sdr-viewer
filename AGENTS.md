# Agents guide

SDR Viewer: a Rust TUI for the uConsole (CM4, aarch64) with the AIO V2
board. It watches the air through an RTL-SDR and shows what a person would
miss. The agreed plan is [plans/PLAN.md](plans/PLAN.md) — read it before
changing behaviour; agreed decisions go into its section 12. Docs, UI
strings and code comments are in English; the user talks to agents in
Russian. The shared conventions are in `~/projects/CLAUDE.md` (written for
web apps: TDD, scripts, docs, license and branches apply here too).

## Commands

- `./scripts/check.sh` — `fmt --check` + `clippy -D warnings` + tests, the
  same as CI. Must pass before any work is called done.
- `cargo run --release [-- FILE --rate 2.4M --center 433.92M --loop]` — the
  app; without a file the demo generator runs.
- `cargo run --release -- --rtlsdr --center 433.92M [--gain 30] [--rate 2.4M]`
  — receive from an RTL-SDR. **On the user's uConsole** only through
  `~/sdr-viewer-check/sdr-viewer-rx.sh ARGS` (below).
- `cargo run --release -- [FILE …] --snapshot 160x45 [--secs 10]` — no
  terminal: analyses the start of the stream unpaced, prints the spectrum
  screen as text and the max-hold peaks. The way to look at a recording or
  at the picture from a script.
- `scripts/smoke.py [--size 80x24] [--keys '12?'] [--secs 5] [-- ARGS]` —
  the release binary in a real pty: presses keys, quits, fails on a crash or
  non-zero exit, prints the CPU used. `--bin PATH` runs a given binary (on
  the uConsole: only Python 3 needed).
- `scripts/hub-speed.sh` — installed on the device as
  `/usr/local/sbin/sdr-hub-speed` (root): `high [--while-pid PID]` puts the
  hub at 480M for the RTL-SDR and unbinds the kernel's DVB driver, `full`
  returns to 12M, `status`. Works through the flag `/run/sdr-viewer-high-speed`
  that the device's udev rule checks, so a reboot always gives 12M.
- `scripts/sdr-viewer-rx.sh ARGS` — on the device: `high` for as long as
  `sdr-viewer --rtlsdr ARGS` runs, then back. The built-in keyboard is dead
  meanwhile (about 6 s to switch each way); say so before running it.
- `scripts/hs-probe.sh` — superseded by the two above; kept for its
  `rtl_sdr`/`rtl_test` cross-check with librtlsdr.
- `scripts/term-check.sh` — can this terminal show truecolor, bars, half
  blocks, Braille (phase 0, on the device).
- `cargo test --release -p sdr-core -- --ignored --nocapture bench_` — cost
  of the generator per sample.

CI (`.github/workflows/ci.yml`): `check.sh` on Linux x86-64; tests and a
static `aarch64-unknown-linux-musl` release build on a native arm64 runner,
uploaded as the artifact `sdr-viewer-aarch64`.

## Plans

| File | What |
|---|---|
| [plans/IDEA.md](plans/IDEA.md) | The original idea, as first written |
| [plans/PLAN.md](plans/PLAN.md) | The agreed plan: screens, architecture, phases, decisions |
| [docs/hardware-check.md](docs/hardware-check.md) | Phase 0 checklist to run on the uConsole |
| [docs/hardware.md](docs/hardware.md) | What phase 0 found on the device so far |

Phase status: 0, 1 — done; 2 — the screen works on the generator, files
and the RTL-SDR; zoom, gain and frequency keys are still to do (the defective hub is forced
to 12 Mbit/s on purpose and the RTL-SDR works only at 480M, where the
keyboard dies; never undo the workaround or run `rtl_*` tools there
without asking), findings in `docs/hardware.md`; 1 — done.
The device is reached with `ssh dmw@192.168.1.246` (key login; sudo works
without a password — install or change only what the user allowed); our files there live in `~/sdr-viewer-check/`.

## Layout

A Cargo workspace under `crates/` (the plan's section 4 lists all eight;
a crate is added in the phase that needs it):

- `sdr-core` — `Source` trait and `Iq` samples; `generator` (seeded tones,
  gated bursts, noise), `file` (raw IQ formats, looping), `pace` (real-time
  playback, `Clock` trait), `rate` (`RateMeter`), `units` (parse/format Hz).
  `frame` (`SpectrumFrame`: dBFS per bin, lowest frequency first).
- `sdr-dsp` — `analyzer`: Hann window, FFT, power averaged over a number
  of blocks into one `SpectrumFrame`.
- `sdr-rtl` — `RtlSource`: the RTL-SDR through `rtl-sdr-rs` (pure Rust on
  `nusb`), queued transfers, gain to the tuner's nearest step, retuning.
- `sdr-tui` — `app` (state and keys, no drawing), `ui` (frame of the
  interface, `render_text`), `spectrum` (the spectrum screen's state and
  drawing arithmetic: columns, scale, palette), `spectrum_ui` (drawing it).
  Snapshot tests in `src/snapshots/`.
- `sdr-viewer` — the binary: `cli` (arguments → source), `reader` (the
  source thread: read, analyse, send `Update`s), `snapshot` (`--snapshot`),
  `main` (terminal loop, redraws only when something changed).

## Invariants

- **TDD**: red → green → refactor; a bug fix starts with a failing test.
- Time, randomness and I/O are parameters (`Clock`, the generator's seed,
  `Read + Seek`), so tests are exact and never sleep.
- The interface never blocks reception: the reader thread `try_send`s its
  status and drops what the UI has no room for.
- Sources that are not hardware are wrapped in `Paced`; hardware paces
  itself.
- Levels are dBFS: a full-scale tone on a bin reads 0. Power is averaged,
  never decibels. Frames run from the lowest frequency up, the centre is
  bin `len / 2`.
- The palette's brightness only grows with the level (a test holds it).
- `unsafe_code = "forbid"` across the workspace.
- `ui` snapshots change only on purpose: review `cargo insta review` (or
  `INSTA_UPDATE=always cargo test` and read the diff) before committing.
- Each feature gets its own branch and a pull request.

## What costs what

Measured on the dev Mac (Apple Silicon), release build:

| What | Cost |
|---|---|
| `./scripts/check.sh` after touching `sdr-core` | ~2 s |
| Generator, demo scene (2 tones + noise) | ~16 ns/sample = ~4% of a P-core at 2.4 MS/s; noise is 1.7, tones 10.5 |
| App with the generator, spectrum screen at 160×45 (smoke) | ~7% of one core, ~0.9 MB/s to the terminal |
| Live reception on the CM4, RTL-SDR at 2.4 MS/s, spectrum at 160×45 | **31%** of one core (0.6 s of 15 in the kernel); 30 s of samples arrive in 30.0 s; opening the device takes 1.15 s |
| The same on the CM4 | **37%** of one core with the generator, **25%** playing a `.cu8`; ~1 MB/s to the terminal. Not split yet into FFT, drawing and escape codes; the terminal emulator's own cost is not measured (the smoke has no window) |
| Generator alone in the app, before the spectrum (phase 1) | 6%; **8% on the CM4** |
| App playing a `.cu8` file | ~2% of one core |
| Release binary (macOS) | 1.3 MB |

Before phasors and the noise table the generator took 70% of a CM4 core:
measure on the device, the Mac's numbers do not scale (`docs/hardware.md`).
