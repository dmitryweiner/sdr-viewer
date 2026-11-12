# SDR Viewer — implementation plan

Status: **agreed**; decisions are in section 12. The open questions in
section 13 do not block the start.

## 1. Goal

A console (TUI) application in Rust for the uConsole with the HackerGadgets
AIO V2 board. The app does not just draw a spectrum: it **watches the air
itself** and shows the person what they would otherwise miss — when, where
and how often something was transmitted.

Main principle: every screen answers one specific question.

| The user's question | Screen |
|---|---|
| What is on the air right now? | Spectrum + waterfall |
| What happened while I wasn't looking? | Signal log, channel strip |
| Which frequencies are busy at all? | Spectral density, panorama (sweep) |
| Does this signal repeat? With what period? | Periodicity graph |
| Has something new appeared that wasn't there before? | Novelty detector, alerts |
| Is my mesh network alive? | Meshtastic pulse |
| What exactly is being sent? | Decoders |

## 2. Hardware and its limits

The facts below were collected from the ClockworkPi forum and must be
checked on the real device (phase 0).

| Module | What it is | How it is connected | Note |
|---|---|---|---|
| RTL-SDR | RTL2832U + R860/R820T2 tuner | USB | 8 bit, ~2.4 MHz of bandwidth at a time, ~24–1766 MHz |
| LoRa | SX1262 with TCXO | SPI (`spidev1.0`, IRQ 26, BUSY 24, RESET 25) | One owner: either `meshtasticd` or us |
| GPS | GNSS module | UART | Time and position |
| RTC | Battery-backed clock | I2C | Time without a network |

Three consequences that shape the architecture:

1. **Module power is off by default.** On the V2 the modules are switched on
   through GPIO (GPS — 27, LoRa — 16, SDR — 7, internal USB — 23; the stock
   tool is `hackergadgets/aiov2_ctl`). The app must check this at start-up
   and explain clearly what to do, and ideally switch them on itself.
2. **The RTL-SDR sees only ~2.4 MHz at once.** "I was looking at the wrong
   frequency" is solved not by a wide view but by a sweep — retuning across
   a range. The price: during a sweep, short bursts elsewhere are lost.
   Hence two modes: "stay and listen" and "walk the range".
3. **Only one process can open the RTL-SDR.** External decoders (rtl_433
   and the like) cannot take the device in parallel — our app has to feed
   them the data.

The uConsole screen is 1280×720. With an 8×16 font that is about 160×45
cells; the interface is designed for that size, with 80×24 as the minimum.

## 3. Stack

| Task | Choice | Fallback |
|---|---|---|
| TUI | `ratatui` + `crossterm` | — |
| RTL-SDR access | `rtl-sdr-rs` (pure Rust, no C dependencies) — confirmed on the device with the R820T | FFI to `librtlsdr`; an `rtl_tcp` client |
| FFT | `rustfft` / `realfft` | — |
| Threads and channels | `std::thread` + `crossbeam-channel` | — |
| Meshtastic | `meshtasticd` TCP API (port 4403) + `prost` | Driving the SX1262 directly (see 7.2) |
| GPS | `gpsd` (JSON over TCP) | Reading NMEA from the UART directly |
| Storage | SQLite (`rusqlite`, bundled) | — |
| Configuration | TOML (`serde`) | — |
| IQ recording | SigMF (`.sigmf-data` + `.sigmf-meta`) | — |
| Logs | `tracing` to a file (stdout belongs to the UI) | — |

No async runtime (tokio): there are few threads, and DSP is computation,
not I/O.

## 4. Architecture

```
 Source ──IQ──▶ DSP ──spectrum frames──▶ ┬─▶ Waterfall ring buffer ─────┐
 (rtl-sdr,                               ├─▶ Event detector ──▶ SQLite  │
  file,                                  ├─▶ Accumulators (density,     ├─▶ UI
  generator)                             │    channels, envelopes)      │
        │                                └─▶ Novelty detector ─▶ alerts ┘
        └──IQ──▶ Decoders (built-in / external processes) ──▶ messages ─▶ UI

 meshtasticd ──TCP──▶ Mesh client ──▶ nodes, packets ──▶ UI
 gpsd ──────────────▶ time, position ──▶ event stamps
```

Key decisions:

- **`Source` is a trait.** Three implementations from day one: RTL-SDR,
  IQ file playback, a synthetic generator. This lets us develop and test
  without hardware and debug the detectors reproducibly.
- **The unit of exchange is a spectrum frame** (a vector of powers in dB +
  a timestamp + the centre frequency). All analysers work on frames, not raw
  IQ. Raw IQ is needed only by decoders and recording.
- **The UI never blocks reception.** Channels are bounded; if drawing falls
  behind, frames are dropped for the screen, not for the analysis.
- **An IQ ring buffer** of N seconds — so that an event can save a recording
  "from the start", including the moment before the trigger.

Layout — a Cargo workspace:

```
crates/
  sdr-core/      Source, frame types, ring buffers
  sdr-dsp/       FFT, windows, averaging, autocorrelation
  sdr-detect/    noise floor, event detector, novelty, occupancy statistics
  sdr-decode/    decoders and the bridge to external processes
  sdr-mesh/      Meshtastic client
  sdr-store/     SQLite, SigMF
  sdr-tui/       screens and widgets
  sdr-viewer/    the binary, configuration, wiring everything together
```

## 5. Screens

Screens are switched with digits (F-keys are awkward on the uConsole
keyboard). Common keys: arrows / trackball — frequency cursor, `+`/`-` —
zoom, `g` — gain, `f` — enter a frequency, `b` — bookmarks, `r` — record,
`?` — help. The keys as built are listed in the README.

### 5.1. Spectrum and waterfall (main)
- Spectrum: bars of `▁▂▃▄▅▆▇█`, with max-hold and average traces on top.
- Waterfall: half blocks `▀` with background and foreground colours — two
  "pixels" per cell vertically. Truecolor palette.
- Noise-floor line, bookmark labels, a cursor with frequency and level.

### 5.2. Channel strip
Name or frequency on the left, occupancy history on the right
`···▂██▃····▅█··`. All rows are aligned in time. Channels come from the
frequency plan (config) or are created automatically where the detector
finds activity. On the right: occupancy share and time since the last
activity.

### 5.3. Spectral density
Frequency across, level up, brightness — how often the signal reached that
level. A 2D histogram with decay over time. It shows rare short signals
well, which are invisible on an averaged spectrum.

### 5.4. Periodicity
Autocorrelation of the selected channel's envelope. Delay across; the cursor
jumps to the main peak and labels it ("2.04 s"). Drawn with Braille dots.

### 5.5. Signal log
One row per event: start time, centre frequency, duration, width, peak
above noise, classifier label. Sorting and filters. Selecting a row shows
the waterfall fragment from the ring buffer and a "save IQ" button.

### 5.6. Meshtastic pulse
One row per node, a tick on the timeline for every received packet. On the
right — time since the last packet and SNR. Packet card: type, RSSI and SNR
of the last hop, hop count (`hop_start − hop_limit`), relay.

### 5.7. Panorama (sweep)
An overview of a wide range (e.g. 430–440 MHz or 24–1700 MHz) accumulating
max-hold and occupancy. Enter on a segment — go there in "stay and listen"
mode.

### 5.8. Decoders
A feed of decoded messages from all active decoders.

## 6. More ideas ("what else would help")

Ordered by value for the effort.

1. **Novelty detector.** The app learns the "usual" picture (background by
   frequency and time of day) and highlights what was not there before.
   This is the direct answer to the original problem.
2. **Alerts.** Rules like "a signal in 433.9–434.0 longer than 1 s and above
   20 dB" → blinking status line, sound, IQ recording.
3. **Recording on event.** Automatically save IQ with the pre-history from
   the ring buffer. The recording can be examined at leisure later.
4. **Sweep with memory** (screen 5.7).
5. **Frequency plan and bookmarks.** Labels on the spectrum: "PMR 3",
   "LPD 17", "airband", "Meshtastic". An unknown signal stands out from a
   known one at once.
6. **"What is this?" card.** For the selected signal: width, duty cycle,
   period, a guess at the modulation (continuous carrier / AM / FM / OOK /
   FSK / LoRa chirp) and a list of matching decoders.
7. **Occupancy by time of day.** An "hour × channel" heat map over days and
   weeks — transmission schedules become visible.
8. **"While you were away" summary.** On returning to the device: N new
   events, M new frequencies, the most active channels.
9. **GPS tagging.** Coordinates in every event; export to CSV/GeoJSON. Lets
   you see where a signal is stronger while moving with the device.
10. **Listening.** AM/NFM/WFM demodulation to the speaker or headphones.
11. **Session replay.** Playing back a saved session with seeking.
12. **Comparison with a reference.** Freeze the spectrum and see the
    difference — handy for hunting interference and "bugs".
13. **Power management.** Switching the AIO V2 modules on from the app,
    battery charge in the status line (reception drains the battery
    noticeably).

## 7. Decoding and decryption

These are two different steps: **demodulation and decoding** (from the
radio signal to bytes) and **decryption** (from encrypted bytes to
meaning). Most interesting signals in this range are not encrypted at all.

### 7.1. What an RTL-SDR can really receive

| Signal | Frequency | Encryption | Approach |
|---|---|---|---|
| ISM sensors (weather, TPMS, remotes) | 433 / 868 MHz | none | external `rtl_433` |
| ADS-B (aircraft) | 1090 MHz | none | own decoder or `dump1090` |
| AIS (ships) | 162 MHz | none | own decoder |
| APRS / AX.25 | 144.8 MHz | none | own decoder (AFSK 1200) |
| POCSAG (pagers) | various | none | own decoder |
| FM RDS | 88–108 MHz | none | own decoder |
| LoRa / Meshtastic | 433 / 868 MHz | AES-CTR | see 7.2 |

Proposed order: first a **bridge to external decoders** — the app feeds
them IQ over stdin and reads JSON (a standard mode of `rtl_433`, which knows
hundreds of protocols). Then own Rust decoders for the most useful
protocols.

### 7.2. Meshtastic

How the encryption works:

- The packet header (16 bytes: destination, sender, id, flags, channel
  hash, relay) **is sent in the clear**. So the "pulse" works for any nodes
  in range, even with unknown keys.
- Channel payloads are encrypted with AES-CTR (128 or 256 bit) using the
  channel key. The nonce is built from the packet id and the sender number.
- The default channel (`LongFast`) uses a **publicly known key** — anyone
  can read its traffic.
- Direct messages in recent firmware are encrypted with public keys
  (Curve25519). Other people's direct messages cannot be read.

Two ways to get packets:

| | A. Through `meshtasticd` | B. Directly from the SX1262 |
|---|---|---|
| How | TCP API, protobuf | own SPI driver, receiving every frame |
| Pros | simple, the node stays a member of the network, a node database exists | we see every packet, including other channels and undecrypted ones |
| Cons | we see only what the daemon hands out | the node stops being a member of the network; a lot of work |

Recommendation: **A in the first version**, B later as a separate "sniffer"
mode. Channel keys for decryption are set by the user in the config.

A third way — software LoRa reception on the RTL-SDR — is possible (the
125–250 kHz bandwidth fits), but laborious and less sensitive than the
SX1262. Not in the plan.

### 7.3. Boundaries

The app decrypts only what the user has provided a key for, and traffic
with publicly known keys. Key guessing and bypassing encryption are not
planned. The rules on receiving and disclosing other people's messages
differ by country — that is the user's responsibility.

## 8. Phases

Every phase ends with a working program.

| # | Phase | Result |
|---|---|---|
| 0 | Hardware check | A note: which uConsole module, which tuner, does `rtl-sdr-rs` work, how power is switched on, colours and fonts in the terminal |
| 1 | Skeleton | Workspace, CI, `Source` (generator + file), an empty TUI with tabs |
| 2 | Spectrum and waterfall | Screen 5.1 on synthetic data and files, then on the RTL-SDR |
| 3 | Detector and log | Noise floor, events, SQLite, screen 5.5 with a waterfall fragment |
| 4 | Accumulators | Channel strip, density, periodicity (5.2–5.4) |
| 5 | Meshtastic | `meshtasticd` client, screen 5.6 |
| 6 | Sweep and novelty | Panorama 5.7, novelty detector, alerts, recording on event |
| 7 | Decoders | Bridge to `rtl_433`, screen 5.8, then own decoders |
| 8 | Polish | GPS, bookmarks, sound, occupancy by time of day, `.deb` package |

The first useful version comes after phase 3.

## 9. Testing

- **DSP and detectors** — unit tests on synthetic data with a known answer
  (a tone at a given frequency, bursts with a given period).
- **Regressions** — a set of short IQ recordings of real signals in the
  repository (or Git LFS) on which the detector must find known events.
- **TUI** — render snapshots through `ratatui::TestBackend` + `insta`.
- **Decoders** — reference recordings with known contents.
- **On the device** — a manual checklist for every phase and a CPU load
  measurement.

## 10. Build and delivery

- CI (GitHub Actions): `fmt`, `clippy`, tests; tests and a release build
  for `aarch64-unknown-linux-musl` on a native arm64 runner (no `cross`
  needed). Building on the device itself must work too.
- Release artefact: a statically linked (musl) binary, so it runs on any
  uConsole image whatever its glibc; later a `.deb` with udev
  rules for the RTL-SDR and a blacklist for the `dvb_usb_rtl28xxu` kernel
  module (without it the kernel grabs the device).

## 11. Risks

| Risk | What we do |
|---|---|
| `rtl-sdr-rs` does not get on with the R860 tuner | Check in phase 0; fallback — `librtlsdr` |
| CM4 performance at 2.4 MS/s | Configurable FFT size and frame rate; measure in phase 0 |
| The SX1262 cannot be shared with `meshtasticd` | Work through the daemon in the first version |
| 8-bit ADC: weak signals next to strong ones get lost | Manual gain, overload indicator |
| False detector triggers | Adaptive threshold, hysteresis, minimum duration; tuning on recordings |
| Growth of the database and IQ recordings | Limits and automatic clean-up |

## 12. Decisions

| Question | Decision |
|---|---|
| Compute module | CM4, 8 GB RAM (measured, `docs/hardware.md`); build target `aarch64-unknown-linux-musl` |
| Where it runs | A terminal emulator: truecolor and any Unicode characters; no mode for the bare Linux console |
| Region | Israel (see below) |
| External decoders | Allowed; the `rtl_433` bridge is an optional feature |
| Phase order | As in section 8: the signal log (phase 3) before Meshtastic (phase 5) |
| Meshtastic | Through `meshtasticd` (the default, no objections) |
| Defective USB hub | The user's device needs its hub at 12 Mbit/s for the keyboard and at 480 for the RTL-SDR (`docs/hardware.md`). Until the mainboard is replaced: a "reception without the keyboard" mode that switches the hub for the time of reception; control over SSH or Bluetooth |
| Mesh network | Haifa: region US, preset MediumFast, frequency slot 70 = 919.375 MHz, max hops 7. Goes into the default bookmarks; the app still reads the frequency from `meshtasticd` |
| Language | UI in English; docs, README and code comments in English (the shared conventions in `~/projects/CLAUDE.md`) |
| aarch64 build | Static musl binary built on GitHub's native arm64 runner instead of `cross` |
| Weak one-frame events | The detector's threshold stays at 8 dB and everything it finds goes into the database; the log hides the weak ones with a filter (minimum level above the floor) whose threshold the user changes from the log screen |
| How long events are kept | 7 days: older events are deleted when the app starts. Nothing is filtered on the way in (~93 bytes an event, ~660 MB a day on a band as busy as the LPD recording, so the file settles under ~5 GB there) |
| Phase order on start | Phase 1 before phase 0: no SSH access to the device yet; phase 0 follows `docs/hardware-check.md` when there is |

### Region: Israel

- Meshtastic has no separate Israel region, and community members disagree
  on which settings are legal. So the app **does not hard-code** the LoRa
  frequency: it is read from `meshtasticd`, and in the frequency plan it is
  set in the config.
- The app only receives and transmits nothing. Configuring the
  `meshtasticd` transmitter (frequency, power, duty cycle) stays with the
  user.
- Default frequency plan (to be refined in phase 0 against the real air):
  FM 87.5–108 MHz, airband 118–137 MHz, amateur 144–146 and 430–440 MHz,
  marine VHF 156–162 MHz (AIS 161.975 / 162.025), ISM 433.05–434.79 MHz,
  the 915–920 MHz band, ADS-B 1090 MHz.

## 13. Still open

They do not block the start; until answered, the default holds.

1. **The terminal's size.** The device's default font gives 79 × 20
   cells (`docs/hardware.md`), below the 80 × 24 minimum. A smaller font
   by default, or a layout that works from 79 × 20?
2. **Which decoders matter most?** *Default: the order in 7.1.*
3. **Sound (listening to AM/FM).** *Default: yes, in phase 8.*
4. **Running in the background (daemon + TUI client).** *Default: not in
   the first version.*
5. **License.** The repository has GPLv3, which is compatible with GPL
   dependencies (`rtl_433`, `librtlsdr`). *Default: stay on GPLv3.*

## 14. Progress

As of 2026-11-12.

| # | Phase | State |
|---|---|---|
| 0 | Hardware check | Done: `docs/hardware.md`. CM4 with 8 GB, R820T tuner, `rtl-sdr-rs` works, truecolor terminal. The device's hub is defective: see the decision "Defective USB hub" |
| 1 | Skeleton | Done: workspace, CI with a static aarch64 binary, generator and file sources, tabs |
| 2 | Spectrum and waterfall | Done on the generator, recordings and the RTL-SDR: bars with max hold, waterfall, a scale that follows the noise floor, cursor, tuning (`f`, `[` `]`, Enter), gain, zoom, pause, `--snapshot` |
| 3 | Detector and log | In progress. Done: the noise floor per bin, the event detector on 100 frames a second, events listed by `--snapshot`, screen 5.5 with the level filter, sorting and tuning to an event (events in memory, stream time), the SQLite event database (`sessions`, `events`; written once a second by its own thread; kept by default only for the RTL-SDR). Events older than 7 days are deleted at the start; with a database the log shows the local time of day and starts with the week's history. Next: the waterfall fragment and "save IQ" of an event, the floor line on the spectrum |
| 4–8 | | Not started |

Left in phase 3, in the agreed order:

1. **The noise-floor line on the spectrum** — the detector's floor per
   bin, sent to the interface with the frames; the spectrum's own estimate
   (which sets the scale) stays or goes once the line is there.
2. **The waterfall fragment of the selected event and "save IQ"** — needs
   the IQ ring buffer of section 4, so that an event has its samples from
   before the trigger. Not started; SigMF for the saved file.
3. **A short reference recording in the repository** — see below.
4. **An overload indicator and the count of dropped transfers.**

Known limits of what phase 3 has built so far:

- The log's time of day uses the UTC offset taken at the start: a change
  to or from summer time while the app runs is not followed.
- Without a database (a recording, the generator) the log shows stream
  time and has no history.
- The log holds the 10 000 newest events in memory; older ones are only
  in the database, and nothing in the app pages through them yet.
- The classifier label of screen 5.5 does not exist: nothing classifies.
- Receiving from the RTL-SDR with the database has not been run on the
  device; the pieces were checked there on the recording and the
  generator with `--db`.

Left out of phase 2 on purpose, with where it goes:

- **Bookmark labels and the frequency plan on the spectrum** — phase 8
  with bookmarks, as section 8 says; needs the TOML config, which nothing
  has needed yet.
- **The noise-floor line and the average trace** — phase 3 (item 1
  above): the detector computes the floor per bin, and the line should
  show that one, not a second estimate.
- **An overload indicator and the count of dropped transfers** — phase 3,
  next to the detector's own health numbers.
- **The terminal emulator's cost on the device** — not measured (the smoke
  has no window); the user reports the picture does not stutter at 10
  frames a second.
- **A short reference recording in the repository** — phase 3, cut from
  `capture-433.cu8` on the device once the detector says which second has
  a known event.

How the detector works (`crates/sdr-detect`), as built:

- One pass of FFTs: 12 to a detector frame (10.24 ms at 2.4 MS/s), 10 of
  those averaged into a screen frame (102.4 ms, so 9.8 rows a second).
- The floor: the median of each of 32 segments of the spectrum, followed
  over time with a 10 s rise and a 0.5 s fall, interpolated per bin. A
  signal wider than half a segment (37 kHz) lifts it, slowly.
- An event: bins more than 8 dB above the floor, joined when closer than
  5 kHz, followed until silent for 100 ms. Its band and peak are those of
  its strongest frame; the union over all frames is used only to match.
- A carrier keyed inside an FFT block splashes over the whole band; what
  is weaker than a signal of the same frame by 10 dB + 20 dB per decade of
  bins is dropped as its splash. This hides weak signals next to a strong
  one while it is on.
- Known limits: the width is measured down to the threshold, so a strong
  pure tone reads ~10 kHz wide (the window's skirts); a signal that never
  ends is one open event forever.

On the device's recording (`capture-433.cu8`, 10 s) it finds 824 events:
most are 40–50 ms long and ~9 kHz wide, exactly on the 25 kHz LPD grid
over some 66 channels — a frequency hopper nobody had noticed on the
waterfall. 90 are one bin for one frame at 8–10.5 dB above the floor: more
than Gaussian noise gives at this threshold, so either the edges of weak
bursts or noise that is not Gaussian. Decided (section 12): the log hides them with a
filter, the threshold stays.

What phase 3 started from: `SpectrumFrame`s at 10 a second from any
source; 100 ms frames average a short burst down (a 10 ms burst reads
10 dB low), so the detector will likely want its own, faster frames from
the same `Analyzer` — measure on the CM4 first.
