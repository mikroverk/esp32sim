# EX210: skip the WiFi air step when nothing can be due

**Question.** Sharing the WiFi code between the chips (`a8478363`, `esp_soc::wifi::StationLink`)
kept the S3 and C3 bit for bit and the goldens unchanged, but made the WiFi station runs about 1–2%
slower in CPU time (+0.6 to +2.8% across runs 1–4 below). While WiFi is on, the air step runs every
scheduling round, millions of times per emulated second, so a nanosecond there shows. Can that path
be made cheaper than before, without changing behaviour? Related: [EX202](../../experiments.md#ex202)
(the C3's cached work flag that schedules the WiFi steps) and [EX134](../../experiments.md#ex134)
(the tick-deferral cadence guard that WiFi activity holds).

**Change.** `f583a23d`: `StationLink::rx_idle`, renamed `nothing_due` in the review follow-up.
Within the airtime gap, or with nothing queued at the access point, no beacon due and nothing from
the network, `next_rx` has nothing to deliver and the access point's `step` changes nothing. The bus
asks this before it reads the receive ring, so those rounds skip the ring read and `step`. Exact by
construction: `step` with an empty queue before `next_beacon_us` returns nothing and changes no
state, and with `eth_rx` empty there is nothing from the network. The review follow-up moves the
access point's half into `VirtualAp::idle_at`, next to `step`, and adds a unit test
(`nothing_due_means_next_rx_delivers_and_changes_nothing`) that sweeps times and states; it fails
when `idle_at` ignores the queue or uses `<=` for the beacon time.

**Arms.** `before` = `9c19f918` (the WiFi station goldens; the old per-chip code); `shared link` =
`a8478363`; `rx_idle` = `f583a23d`; `review follow-up` = the commit that adds run 4, on top of
`9107fdc3` (private `StationLink` state, `attach_wifi` taking the configuration, `now_us` helpers).
Release builds, rustc 1.98.1, fat LTO, one codegen unit; Apple M5 Max, macOS 26.6.2, one-minute load
1.8–3.2 during runs 1–3 and 3.2–5.1 during run 4.

**Method.** User CPU seconds of the whole process (`/usr/bin/time -p`, 10 ms resolution, about 1.3%
of an S3 WiFi sample), arms alternating within each round, medians. `bench.sh WORKLOAD BIN_DIR
ROM_DIR [MODEL]` takes one sample, run from the repository root; it fails if a WiFi run did not get
its five pings. WiFi workloads: the committed station firmware, 14 s emulated (the same runs as
`wifi_station_{s3,c3,c6}`). Idle workloads: S3 hello for 3000 s, C3 and C6 hello for 30 s, and
pocket-tank for 30 s. Runs 1–3 used the WiFi firmware as committed in `9c19f918`; run 4 used the
reproducible rebuild from `24661fd9` for every arm. Every sample, the instruction counts (identical
in every sample of a workload) and the SHA-256 of every input are in `samples.json`.

**Run 4** (all arms on the same firmware; median user seconds, change against `before`):

| workload | before | shared link | rx_idle | review follow-up |
| --- | --- | --- | --- | --- |
| WiFi station S3 (10 samples) | 0.810 | 0.815 (+0.6%) | 0.750 (−7.4%) | **0.755 (−6.8%)** |
| WiFi station C3 (10) | 1.625 | 1.650 (+1.5%) | 1.520 (−6.5%) | **1.510 (−7.1%)** |
| WiFi station C6 (10) | 2.065 | 2.100 (+1.7%) | 1.950 (−5.6%) | **1.960 (−5.1%)** |
| S3 hello, 3000 s (3) | 25.85 | — | 25.87 (+0.1%) | 25.44 (−1.6%) |
| C3 hello (3) | 2.580 | — | 2.560 (−0.8%) | 2.580 (0.0%) |
| C6 hello (3) | 3.530 | — | 3.570 (+1.1%) | 3.590 (+1.7%) |
| pocket-tank (2) | 36.885 | — | — | 36.475 (−1.1%) |

Runs 1–3, with the older firmware, gave the same picture for the WiFi workloads: the shared link
+0.6 to +2.8%, `rx_idle` −7.4% (S3), −6.8% (C3) and −5.8% (C6) in run 3 (20 samples per arm). With
`rx_idle` and in the review follow-up the WiFi ranges do not overlap with `before` on any chip.

**Correctness.** Every arm passes all CI goldens unchanged (15 golden runs, among them the three WiFi
station goldens: console, station lines, instruction count, WiFi, network and interrupt counts). The
S3 Linux demo (`linux-login`, 40 s, which drops 12 frames for want of a descriptor) gives the same
console (SHA-256 `4b1c65f0f237…`), 34,140,713 + 462,299,162 instructions, 601,889 exceptions and
11,100 interrupts natively for `before`, `shared link`, `rx_idle` (checked in review) and the review
follow-up, and 496.4 M instructions with 174 console lines in the WebAssembly test for `before`,
`shared link` and the review follow-up.

**Uncertainty.** The WiFi runs are short (0.75–2.1 s), so process start-up is part of every sample,
and the timer's 10 ms resolution is close to the shared link's own +0.6% on the S3. The idle
workloads move within ±1.7% with opposite signs between runs (C6 hello −1.4% in run 3, +1.7% in run
4), which is the noise of 2–4 samples; they do not run the WiFi code. The cause of the shared
link's own 1–2% is not established (the per-round logic is the same; code generation is the likely
difference). Browser speed was not measured.

**Adoption.** Adopted with the shared link (PR #192).
