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
- `scripts/smoke.py [--size 80x24] [--keys '12?'] [--secs 5] [-- ARGS]` —
  the release binary in a real pty: presses keys, quits, fails on a crash or
  non-zero exit, prints the CPU used. `--bin PATH` runs a given binary (on
  the uConsole: only Python 3 needed).
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

Phase status: 0 — waiting for access to the device; 1 — done.

## Layout

A Cargo workspace under `crates/` (the plan's section 4 lists all eight;
a crate is added in the phase that needs it):

- `sdr-core` — `Source` trait and `Iq` samples; `generator` (seeded tones,
  gated bursts, noise), `file` (raw IQ formats, looping), `pace` (real-time
  playback, `Clock` trait), `rate` (`RateMeter`), `units` (parse/format Hz).
- `sdr-tui` — `app` (state and keys, no drawing), `ui` (drawing). Snapshot
  tests in `src/snapshots/`.
- `sdr-viewer` — the binary: `cli` (arguments → source), `reader` (the
  source thread), `main` (terminal loop).

## Invariants

- **TDD**: red → green → refactor; a bug fix starts with a failing test.
- Time, randomness and I/O are parameters (`Clock`, the generator's seed,
  `Read + Seek`), so tests are exact and never sleep.
- The interface never blocks reception: the reader thread `try_send`s its
  status and drops what the UI has no room for.
- Sources that are not hardware are wrapped in `Paced`; hardware paces
  itself.
- `unsafe_code = "forbid"` across the workspace.
- `ui` snapshots change only on purpose: review `cargo insta review` (or
  `INSTA_UPDATE=always cargo test` and read the diff) before committing.
- Each feature gets its own branch and a pull request.

## What costs what

Measured on the dev Mac (Apple Silicon), release build:

| What | Cost |
|---|---|
| `./scripts/check.sh` after touching `sdr-core` | ~2 s |
| Generator, demo scene (2 tones + noise) | ~40 ns/sample = ~10% of a P-core at 2.4 MS/s; noise is 22, tones 15 |
| App with the generator (smoke) | ~25–30% of one core — the reader thread likely runs on an E-core |
| App playing a `.cu8` file | ~2% of one core |
| Release binary (macOS) | 1.3 MB |

On the CM4 (Cortex-A72) expect several times more; phase 0 measures it.
If the generator is too dear there: rotate tone phasors instead of
`sin_cos`, and draw noise from a precomputed Gaussian table.
