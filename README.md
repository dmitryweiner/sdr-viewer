# SDR Viewer

A console app for the [ClockworkPi uConsole](https://www.clockworkpi.com/uconsole)
with the HackerGadgets AIO V2 board. It watches the air through the board's
RTL-SDR and shows what a person would otherwise miss: when, where and how
often something was transmitted.

**Status: phase 5 of 8 built, phase 6 begun** — the spectrum and waterfall
screen, the channel strip, the spectral density, the periodicity screen and the signal log work on the built-in
signal generator, on IQ recordings and on an RTL-SDR: tuning, gain and
zoom from the keyboard, detected events kept in a database, their pictures
and their samples, alerts on the user's rules. The Meshtastic pulse listens to `meshtasticd`: every
node's packets on one time axis, their texts, batteries and positions,
kept in the same database. The panorama and the decoders are still to
come. See [plans/PLAN.md](plans/PLAN.md) for what comes next.

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

At the right end of the header is the name of the band the middle of the
view is in — `airband`, `amateur 2 m`, `marine VHF`, `PMR446`,
`ISM 433 / LPD` — and when the cursor has crossed into another band, that
band's name stands beside the cursor's frequency. The names come from a
table in the app (ITU Region 1 as commonly used, `crates/sdr-core/src/bands.rs`):
a hint, not a licence — national plans differ, and between the bands it
knows the app says nothing.

The dotted line over the noise is the detector's threshold, 8 dB above the
noise floor it follows: a bar that crosses it becomes an event in the log.
The header says how far above that floor the peak, or the cursor, is.

The status line at the bottom names the source and the rate samples
arrive at. Beside it, only when there is something to tell: `clip 2.4%` —
the share of samples at full scale, the receiver is overloaded (yellow
from 0.1%, red from 1%: lower the gain with `G`); `lost 0.05 s` — samples
the receiver delivered and the app did not take in time, since the start.

The channel strip (`2`) answers "what happened while I wasn't looking?":
one row per channel, the lowest frequency at the top, and all rows on one
time axis with now at the right. A column is a second; the bar in it is
the share of that second the channel was busy, and the shortest burst
still leaves the lowest bar. On the right: the channel's period when it
repeats for sure (see the periodicity screen below), the share of the
shown time the channel was busy, and how long it has been quiet (`now`
while a signal is on). A channel outside the band being received is
dimmed.

```
LPD 17       ····▂····▂····▂····▂····▂····▂····▂····  5.00 s  6.0%   4 s
433.700 MHz  ·····████████████████████··············           36%  30 s
434.100 MHz  ···························▄███████████           21%   now
```

| Key | What |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select a channel (`Esc` lets go) |
| `+` `-` | fewer, more seconds to a column: 1, 10, 60, 300 |
| `Enter` | tune to the selected channel and show the spectrum |
| `X` `X` | let go of the channels the app made (it asks once): from the strip and from the file; those with a name or `hide` stay, and so do those with a signal on at that moment |

The last 4 hours of this run are remembered.

The receiver's own spike — a few bins wide, exactly on the centre
frequency — is not counted: it is not on the air, and after `Enter` it
would sit on the very channel tuned to. A real carrier that narrow,
exactly on the centre, is not counted either while it is there: tune a
little aside to watch it.

### The channels file

A channel is made by itself where a signal is found that is in none: for
an event at least 10 dB above the noise, as wide as the event and at least
10 kHz (at most 500 are made in a run). With `--rtlsdr` the channels are
kept in `sdr-viewer/channels.toml` under `$XDG_CONFIG_HOME` or `~/.config`
(`--channels FILE` elsewhere — and for a recording or the generator, which
keep none otherwise; `--no-channels` reads and writes no file). The file
is yours to edit:

```toml
[[channel]]
name = "LPD 17"      # optional; without it the row shows the frequency
freq_mhz = 433.475   # the centre
width_khz = 12.5
hide = true          # optional: no row, and no new channel made there
```

The app reads the file when it starts and only ever adds new channels to
its end: your names, order and comments stay as they are. Edit it while
the app is not running, or your changes show only at the next start. To
get rid of a row — the receiver's own spike at the centre frequency, say —
hide the channel instead of removing it, or it is made again. `X` pressed
twice on the strip removes the channels that have neither a name nor
`hide` and no signal on at that moment — after a transmitter beside the
receiver has left dozens of them; give a name to what is to stay. A file that
does not read as TOML stops the app at the start with the line that is
wrong; it is never overwritten.

The periodicity screen (`3`) answers "does this signal repeat, and with
what period?" for the channel selected on the strip (the selection is one
for both screens). At the top is the channel's envelope — when it was on,
now at the right, like its row of the strip. Under it is how like itself
that envelope is after a delay (its autocorrelation): delay runs across,
and a peak at 2.04 s says that what was on is on again 2.04 s later. The
cursor stands on the main peak — the shortest of the strongest — and
names it; the line above lists it with its strength, the share of the
signal that came again after that time, and then the other periods found
(a rhythm inside the rhythm), without their multiples.

| Key | What |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select a channel (`Esc` lets go) |
| `+` `-` | shorter, longer delays: up to 1 s, 10 s, 60 s, 10 min |
| `Enter` | tune to the selected channel and show the spectrum |

The envelope is made of the detector's events, so it is on or off in
10 ms steps, and is six times as long as the longest delay shown: delays
up to 10 s are looked for in the last minute, delays up to 10 min in the
last hour, in steps of half a second. A range shows the period the
channel has in it until `+` or `-` chooses another. Periods are read only
out of an envelope long enough (6 s for the two short ranges, 30 s and
5 min for the long ones), and the strip tells a period only from 60%
strength: two bursts are not yet a rhythm. A carrier that never goes off
has no period here — its level is not looked at — and a channel that was
not being received reads as silent.

The spectral density (`4`) answers "which frequencies are busy at all?":
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
| `X` `X` | erase the log (it asks once): every ended event, those of earlier runs too, and with a database from the database as well; the events on now stay |

The picture of the selected event is a small spectrogram made of its
samples: time runs across, from 50 ms before the event to 50 ms after it
(of a long one, the first 3 s), frequency runs up, four widths of the
event and at least 40 kHz; `▲` marks where the event begins and ends. It
needs the samples to be still held (see below), like saving.

The Meshtastic pulse (`6`) answers "is my mesh network alive?". It
talks to `meshtasticd` over its API (TCP port 4403) — `--mesh` on the
same machine, `--mesh=HOST[:PORT]` on another; the app transmits nothing
itself. **Start `meshtasticd` first**: on the uConsole it does not start
at boot, and `sudo ~/sdr-viewer-check/lora.sh on` powers the LoRa radio
and starts it (`off` stops it). While the daemon does not answer, the
pulse says so and gives the command that starts it (the `lora.sh`
beside the app, or `systemctl`), and connects by itself once it is up.

One row per node the daemon knows or that was heard in the last day —
the node itself first, then the last heard — with its short and long
name, a tick for each of its packets on one time axis of the wall clock
(those of earlier runs too, from the database, before the node answers),
and on
the right how long ago it was last heard (`heard`), and the SNR, RSSI
and hops of its last packet. The axis is on top: what a column stands
for (a minute at first), the time of day, and now at the right edge. The
columns begin on whole minutes (or whatever a column stands for), so at
a minute a column the whole pulse steps left once a minute; `+` makes a
column 30 or 10 s, and it moves faster. A dot is a column the app was
listening in and heard nothing from that node; where there is no dot
the app was not connected. Ticks there are what `meshtasticd` kept for
it meanwhile: at most 100 packets, the first ones — after that it drops
what it hears (`ToPhone queue is full, drop packet` in its log), so a
gap there is not silence.

The node never hears itself. On its own row a tick (in its colour) is a
packet of its own it queued for the air — its node info, a reply, a
text another client sent through it — as `meshtasticd` tells the app;
its acknowledgements go untold. A dim `•` is what `meshtasticd` handed
the app alone, over the API — the node's own telemetry, about every
minute: it was not on the air. The node's card adds its own count of
what it sent on the air, acknowledgements too, which it tells every 15
minutes. `meshtasticd` serves one client at a time: a phone app on the
node's API and the pulse take turns.
The card counts a node's packets in the last 24 hours, or since `m`, and
tells what the node last told of itself: its battery (or "powered"),
voltage, how busy the channel is and its own airtime; where it is (to
the metre or, when its sender rounded it, "±365 m"), and how far that is
from where the node itself is, when it has told that.
Under the list are the last texts of every channel the node reads, the
newest last — those of the last week in the database too:
`09:59  HG1 → all · MediumFast  the gate is open`. While a node is
selected, they are its texts alone.
Packets on a channel the node has no key for count too: their header is
in the clear. The selected node's card names its id, hardware and role,
and tells its last packet: what it was, to whom, on which channel (or the
channel's hash, without a key), how it arrived and which node relayed it
last.

| Key | What |
|---|---|
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | select a node (`Esc` lets go) |
| `+` `-` | less, more time to a column: 10 s, 30 s, 1 m, 5 m, 15 m, 1 h |
| `s` | sort by the last heard, or by hops: the fewest first (0 is heard directly), the unknown last |
| `a` | only the nodes heard in the last 15 minutes, or all |
| `m` | start the list afresh: only the nodes heard from now on, and their packets, as if the app had started now (nothing is forgotten; names stay known) |
| `X` `X` | erase every node (it asks once): the list, the ticks, the texts, and the mesh's packets in the database too. The moment is kept: after a new connection or start only the nodes heard again come back, by their names |

The node itself stays on top whatever is chosen; under the rows a line
tells what the list shows when it is not everything.

`--mesh` alone opens no receiver at all — on the uConsole the SDR and the
LoRa radio are not used together — and the screens of the receiver say
so; with a recording or `--rtlsdr` both run.

```sh
sdr-viewer --mesh                                      # meshtasticd on this machine
sdr-viewer --mesh=192.168.1.246 --snapshot 160x45 --screen 6 --secs 20
```

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

### Alerts

A rule says what is worth an alert: a band the signal's centre is in, how
long it must have been on, how far its peak stands above the noise. The
moment an event meets a rule — while it is still on — the status line
says so, red: the rule's name and how many alerts it gave
(`! gate ×3`, or `! gate ×3 of 5` when other rules gave some too), bright
for 10 s, then dim. A tone, two short beeps, plays through the speaker
with `aplay` (on the uConsole it is there; elsewhere the status line says
why there is none): at most once in 10 s a rule, so a rule over a busy
band does not beep without end. An event gives a rule's alert once,
however long it goes on.

With `--rtlsdr` the rules are in `sdr-viewer/alerts.toml` under
`$XDG_CONFIG_HOME` or `~/.config` (`--alerts FILE` elsewhere — and for a
recording or the generator, which have none otherwise; `--no-alerts`
reads none). A file that is not there is made, with this example in its
comments:

```toml
[[alert]]
name = "gate"          # optional; without it the alert shows the frequency
from_mhz = 433.9       # the band the signal's centre must be in;
to_mhz = 434.0         # or freq_mhz = 433.95 with width_khz = 100
min_ms = 1000          # optional: on for at least this long (0)
min_db = 20            # optional: its peak this far above the noise (10)
```

`!` on the channel strip makes a rule for the selected channel (by its
name, or its frequency); in the log, for the selected event's band (at
least 10 kHz wide). It holds at once, and goes to the end of the file
with every key written, ready to be changed: `min_ms = 0`, `min_db = 10`.
A band that has a rule already gets no second one.

The app reads the file when it starts and only ever adds to its end:
what you wrote stays as it is. Edit it while the app is not running, or
your changes hold only from the next start. A rule that does not read —
a key mistyped too — stops the app at the start with what is wrong.

### The event database

With `--rtlsdr` every detected event is also kept in a SQLite file:
`sdr-viewer/events.db` under `$XDG_DATA_HOME`, or under `~/.local/share`;
with `--mesh` every packet the node hands the app is kept in the same
file. `--db FILE` names another file — and keeps the events of a recording
or of the generator too, which are not kept otherwise; `--no-db` keeps
nothing. Events and packets are written once a second; events still on
when the app quits are ended there and written. What is older than 7 days
is deleted when the app starts. A file this build has brought up to date
(the mesh's tables were added to it, and when it was erased) is refused
by older builds: they say so, and run with `--no-db`.

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

The mesh's tables, times in Unix seconds: `mesh_listened` — when the app
was connected to which node (`until_s` moves on while it is);
`mesh_packets` — each packet's header (when the node heard it by its
clock, from, to, channel or its hash, number, SNR, RSSI, hops taken and
allowed, the last byte of the relay, via MQTT, made for the app alone)
and what it carried as the node decrypted it: its kind (`port`,
Meshtastic's `PortNum`: 1 a text, 3 a position, 4 node info, 67
telemetry) and its `Data` message (`data`), both NULL without the
channel's key; `mesh_sent` — the packets the node queued of its own.

```sh
sqlite3 ~/.local/share/sdr-viewer/events.db "
  SELECT datetime(received_s, 'unixepoch', 'localtime'),
         printf('!%08x', from_node), port, snr, hops
  FROM mesh_packets ORDER BY received_s DESC LIMIT 20"
```

Without a terminal — the screen as text, the strongest signals and the
detected events of the first seconds of a recording:

```sh
cargo run --release -- capture.cu8 --rate 2.4M --center 433.92M --snapshot 160x45 --secs 10
```

`--screen 2` prints the channel strip instead of the spectrum, `--screen 3`
the periodicity of the channel that repeats most surely, `--screen 4` the
density; `--channels FILE` gives a snapshot its channels (it adds none to
the file), `--alerts FILE` its rules, and their alerts are listed after
the events. `--screen 6` with `--mesh` prints the Meshtastic pulse after
`--secs` of the node's talk, and the packets heard; it fails when
`meshtasticd` does not answer.

On the uConsole: take the static `aarch64` binary from the CI artifact
`sdr-viewer-aarch64`. The first run on a device is the
[hardware check](docs/hardware-check.md).

## Develop

```sh
./scripts/check.sh      # fmt + clippy + tests, the same as CI
```

## License

GPL-3.0-or-later, see [LICENSE](LICENSE).
