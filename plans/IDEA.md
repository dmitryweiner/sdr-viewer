# SDR Viewer

The original idea, as first written down. The agreed plan is
[PLAN.md](PLAN.md).

### Description

A console application for the uConsole.
It takes data from the AIO V2 board and shows it as text-mode graphics.
The point is that an SDR receives a lot of data, and the human brain cannot
analyse it all. For example, signals may be transmitted somewhere and a
person misses them because they were looking at the wrong frequency, or did
not recognise them as signals.
The app can help with this by showing metrics that do not let the wanted
signals slip by.

### Requirements

What could be shown:
- The signal spectrum as a histogram
- A waterfall (the spectrum in colour over time)

Wilder ideas:
- Channel strip: the frequency or name on the left, a history bar on the
  right `···▂██▃····▅█··`. All rows are aligned in time.
- Spectral density: frequency across, level up, the brightness of a cell
  shows how often the signal reached that level.
- Periodicity graph: delay across, autocorrelation of the envelope up. A
  cursor labels the peak, e.g. 2.04 s.
- Signal log: one row per event — start time, centre frequency, duration,
  approximate width and peak above the noise floor. Pick a row to see the
  waterfall fragment from the ring buffer.
- Meshtastic pulse: one row per node; every locally received radio message
  leaves a tick on the timeline. On the right — time since the last packet
  and SNR. A packet card can show the RSSI and SNR of the last radio hop.
  This matters for packets that came through a repeater.

What else would help the user?

### Decoding

It would also be good to decode received digital radio signals (and to see
how they are encrypted).

### Details

- Operating system: https://github.com/clockworkpi/uConsole/tree/master#uconsole-os-images
- Stack: Rust
