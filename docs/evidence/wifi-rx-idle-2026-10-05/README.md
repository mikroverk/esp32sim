# EX210: skip the WiFi air step when nothing can be due

**Question.** Sharing the WiFi code between the chips (`a8478363`, `esp_soc::wifi::StationLink`)
kept the behaviour bit for bit but made the WiFi station runs 0.7–2.2% slower in CPU time. While
WiFi is on, the air step runs every scheduling round (millions of times per emulated second), so a
nanosecond there shows. Can that path be made cheaper than before, without changing behaviour?

**Change.** `StationLink::rx_idle`: within the airtime gap, or with nothing queued at the access
point, no beacon due and nothing from the network, `next_rx` has nothing to deliver and the access
point's `step` changes nothing. The bus asks `rx_idle` instead of `rx_gap` before it reads the
receive ring, so those rounds skip the ring read and `step`. Exact by construction: `step` with an
empty queue before `next_beacon_us` returns nothing and changes no state, and with `eth_rx` empty
there is nothing from the network.

**Arms.** `before` = `9c19f918` (the WiFi station goldens, the old per-chip code); `shared link`
= `a8478363`; `shared link + rx_idle` = this change on `a8478363`. Release builds, rustc 1.98.1,
fat LTO, one codegen unit; Apple M5 Max, macOS 26.6.2, one-minute load 1.8–3.2.

**Method.** User CPU seconds of the whole process, alternating arms within each round, medians.
WiFi workloads: the committed station firmware on each chip, 14 s emulated
(`bench-wifi.sh`, the same runs as `wifi_station_{s3,c3,c6}`). Idle workloads: the hello_world
demos on C3 and C6 and pocket-tank on the S3, 30 s each. Every sample is in `samples.json`.

| workload (median user s) | before | shared link | shared link + rx_idle |
| --- | --- | --- | --- |
| WiFi station S3 (20 samples) | 0.810 | 0.815 (+0.6%) | **0.750 (−7.4%)** |
| WiFi station C3 (20) | 1.620 | 1.650 (+1.9%) | **1.510 (−6.8%)** |
| WiFi station C6 (20) | 2.070 | 2.090 (+1.0%) | **1.950 (−5.8%)** |
| C3 hello (4) | 2.560 | — | 2.560 (0.0%) |
| C6 hello (4) | 3.550 | — | 3.500 (−1.4%) |
| S3 pocket-tank (3) | 36.33 | — | 36.27 (−0.2%) |

An earlier two-arm run (20 samples) put the shared link at +1.9% (S3), +2.2% (C3) and +0.7%
(C6) against `before`; the first six-round run had +1.9%, +2.8% and +1.0%, with the idle
workloads within ±1%.

**Correctness.** Each arm passes all 17 CI goldens unchanged, among them the three WiFi station
goldens (console, station lines, instruction count, WiFi, network and interrupt counts). The S3
Linux demo (`linux-login`, 40 s, which drops 12 frames for want of a descriptor) gives the same
console, 34,140,713 + 462,299,162 instructions, 601,889 exceptions and 11,100 interrupts natively
for `before` and `shared link`, and 496.4 M instructions with 174 console lines in the WebAssembly
test for both.

**Uncertainty.** The WiFi runs are short (0.75–2.1 s), so process start-up is part of every
sample; the ranges of `before` and `rx_idle` do not overlap on any chip. The cause of the shared
link's own 1–2% is not established (the per-round logic is the same; code generation is the likely
difference). Browser speed was not measured.

**Adoption.** Adopted on branch `wifi/shared-link`, on top of the shared link.
