# Recordings for tests

| File | What |
|---|---|
| `lpd433.cu8` | 0.16 s of the air at 433.92 MHz: 2.4 MS/s, `cu8` (what `rtl_sdr` writes), RTL-SDR with the R820T tuner at 30 dB, antenna on, Haifa, 2026-11-12. Bytes 23 808 000 to 24 576 000 (from 4.96 s) of the 10 s `capture-433.cu8` kept on the device (`AGENTS.md`, "The user's device"). LPD433 traffic over a −71 dBFS floor: bursts 40–60 ms long and ~9 kHz wide on the 25 kHz grid (433.075 MHz + n × 25 kHz), the strongest at 434.225 MHz from 51 to 113 ms, and the receiver's own spike at the centre. |

The detector's regression test on it is in
`crates/sdr-viewer/src/pipeline.rs`: when the detector changes on purpose,
look at what it now finds (`cargo run --release -- testdata/lpd433.cu8
--snapshot 160x45`) before changing the numbers there.

Cut with:

```sh
dd if=capture-433.cu8 of=lpd433.cu8 bs=768000 skip=31 count=1
```
