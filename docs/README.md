# esp32sim documentation

Three chips, two CPU architectures: the ESP32-S3 (Xtensa LX7), the ESP32-C3 and the ESP32-C6
(RISC-V). Most documents describe the S3; [esp32c3.md](esp32c3.md) and [esp32c6.md](esp32c6.md)
cover the RISC-V side.

| Document | What it covers |
| --- | --- |
| [architecture.md](architecture.md) | How the emulator is built: crates, CPU core, SoC bus, scheduling, boards, UI |
| [peripherals.md](peripherals.md) | Every modelled block, what part of it is modelled, what is missing |
| [boards.md](boards.md) | The `BoardModel` trait, the three boards, pin maps, how to add one |
| [cli.md](cli.md) | Command-line flags, environment variables, action scripts, output files |
| [web-ui.md](web-ui.md) | The browser UI and its WebSocket protocol |
| [decisions.md](decisions.md) | Design decisions and hard-won gotchas (the "why" behind the code) |
| [roadmap.md](roadmap.md) | What is planned, in priority order |
| [wifi-plan.md](wifi-plan.md) | Plan + status: full WiFi emulation with the unmodified blob (MAC model + virtual AP) |
| [wifi-c6-plan.md](wifi-c6-plan.md) | Plan + status: the C6's own WiFi MAC model, its register map and what differs from the S3 |
| [networking-howto.md](networking-howto.md) | How to run firmware with WiFi and the network: flags, what the subnet offers, debugging, limits |
| [networking-plan.md](networking-plan.md) | Status: the emulated subnet and the user-mode NAT that carries traffic to the host network |
| [speed-plan.md](speed-plan.md) | Plan: performance roadmap — measured baselines, block interpreter, JIT, with the rejected ideas |
| [esp32c3.md](esp32c3.md) | The ESP32-C3 (RISC-V) model: what boots, how to run it, and the bring-up gotchas |
| [esp32c6.md](esp32c6.md) | The ESP32-C6 (RISC-V, RV32IMAC) model: the Waveshare LCD-1.47 bring-up, what silicon found, what is left for the board |
| [wasm.md](wasm.md) | How to build and run the emulator in the browser (WebAssembly): `tools/wasm-build.sh`, `?wasm`, manifests, what works, limits |
| [wasm-plan.md](wasm-plan.md) | The plan the browser build followed, with the original measurements; status at the top |
| [testing-plan.md](testing-plan.md) | Plan: test layers, CPU/SoC/board/firmware suites, CI tiers, milestones |

Board-specific material lives next to the board: `../examples/waveshare-cam/README.md`.
The Atech Pocket Synth firmware is its own project: [atech-firmware](https://github.com/joakimeriksson/atech-firmware). The top-level `../README.md` is the quick start.
