# MSBJIT

A Rust runtime that loads Scratch 3 `.sb3` projects, lowers scripts to a validated control-flow graph, and compiles that graph to native code with Cranelift. Execution runs on a dedicated thread. Optional wgpu rendering and local WebRTC streaming run independently of the script tick rate.

Requires Rust 1.93 or newer. The streaming feature also requires a graphics adapter supported by wgpu and an `ffmpeg` executable with an H.264 encoder.

```sh
cargo run --release -- examples/counter.json --turbo --json
cargo run --release -- examples/demo.sb3 --seconds 5
cargo run --release --features streaming -- examples/demo.sb3 \
  --serve 127.0.0.1:8080 --fps 144 --encoder auto
```

Open `http://127.0.0.1:8080` and click **Connect**. Press Ctrl-C to stop the host. To view from another machine on a trusted LAN, bind to `0.0.0.0:8080` and open the host's LAN address. The player uses host ICE candidates with an empty ICE server list; there are no STUN/TURN or third-party signaling services. This server has no authentication and is intended for loopback or a trusted private network.

`--seconds` sets an execution limit and, when streaming, a host duration. `--max-steps` caps executed IR instructions. `--tick-hz` defaults to 30. `--turbo` removes loop frame boundaries without removing explicit waits. `--watchdog-ms` may lower the warp execution budget from its maximum of 500 ms. `--json` prints final variables, sprite state, captured say/think messages, and execution statistics. Compilation time is excluded from execution statistics.

## Execution model

The loader reads bounded ZIP entries directly without extracting them. Variables resolve by Scratch ID with stage fallback. Block graphs are checked for missing links, cycles, and excessive depth. Active unsupported commands/reporters and event hats produce errors with block context; detached workspace stacks are ignored.

Cranelift emits native branches, loop control, and floating-point addition, subtraction, multiplication, and division. Rust helpers provide Scratch coercions, string operations, event dispatch, and state changes. This is a JIT, but dynamic helper calls still have overhead; it does not promise C-equivalent throughput for every Scratch workload.

Values exposed to Rust use `ScratchValue::{Number, String, Boolean}`. The JIT ABI uses NaN-boxed values with unboxed floating-point numbers, interned string handles, and boolean tags. A collector reclaims temporary strings at scheduler boundaries, preserving constants, variables, and procedure arguments.

Each cooperative script retains a program counter, procedure frames, arguments, and repeat counters. Normal loop backedges yield until the next tick. Straight-line code and turbo loops cooperate after a 2 ms slice. Warp custom blocks and their callees skip ordinary yields, while checkpoints enforce cancellation, step/time limits, recursion limits, and the warp watchdog. A watchdog expiration terminates the run with an error. Explicit waits and broadcast-and-wait suspend scripts, including warp scripts. Broadcast receivers restart when their message is received again.

The renderer reads atomic sprite components without locking the execution thread. A snapshot can span multiple state updates; it is not a transactionally consistent whole-project snapshot. GPU rendering/capture occurs on the calling main thread. Bounded queues drop frames when encoding/network delivery falls behind, rather than blocking scripts. FFmpeg input/output have dedicated threads; signaling and WebRTC use Tokio workers.

## Supported blocks

- Green flag and broadcast-received hats; broadcast and broadcast-and-wait.
- Variable set, change, and reporters; sprite-local and stage variables.
- Repeat, forever, repeat-until, wait-until, if, if/else, wait, and all three stop modes.
- Custom block definitions/calls with parameters, defaults, recursion, and inherited warp mode.
- Arithmetic, modulo, comparisons, booleans, random, join, contains, letter, length, round, and standard math reporters.
- Move steps, go to x/y, set/change x/y, point direction, turn, and rotation style.
- Show/hide, set/change size, switch/next costume, costume reporters, say/think logging, and timer/reset timer.
- SVG, PNG, and JPEG costumes, bitmap resolution, rotation centers, sprite rotation/visibility/size, and initial layer order. The stage is 480 × 360.

This is a bounded Scratch subset. Lists, clones, keyboard/mouse/click events, sensing collisions, sound, pen/extensions, glide, effects, backdrop commands, variable monitors, and speech-bubble rendering are not implemented. Say/think text appears in the final report. SVG text/font support is disabled in this build. Raw JSON projects support execution; costume rendering requires assets from an `.sb3` archive. String operators currently use Unicode scalar characters rather than JavaScript UTF-16 code units, so non-BMP text may differ from Scratch. Size/position clamping also differs from Scratch's renderer.

## Streaming and performance limits

`--encoder auto` probes NVENC, VA-API, then software libx264 with an actual one-frame encode. Explicit choices are `nvenc`, `vaapi`, and `software`; `--vaapi-device` selects the render device. Every encoder disables B-frames, uses a one-second keyframe interval, and produces H.264 Annex-B access units. webrtc-rs packetizes those units as RTP/SRTP over UDP. The bundled browser is receive-only; it does not forward project input.

**GPU capture currently uses portable texture-to-buffer readback followed by a CPU pipe to FFmpeg.** DMA-BUF/CUDA/VA-API texture interop and a zero-copy encoder path remain unimplemented. AV1 is not implemented. The spec's sub-millisecond capture, 2–5 ms hardware encoding, and 5–25 ms network budgets are performance targets, not measured guarantees. A 144 Hz capture setting does not guarantee that the available GPU, encoder, network, or browser will sustain that rate.

## Verification

```sh
cargo test
cargo test --all-features
# Optional GPU/FFmpeg/WebRTC integration checks:
cargo test --all-features -- --ignored
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

Runtime tests execute generated native code for arithmetic/coercion, branches, nested loops, frame pacing, warp inheritance/watchdogs, cancellation, explicit waits, procedure arguments, broadcasts, string lifetime, and atomic scene publication. Streaming unit tests cover Annex-B parsing. Opt-in integration tests validate captured pixel colors and GPU → FFmpeg → WebSocket/host ICE → H.264 RTP delivery to a native WebRTC peer. These can run through a software wgpu adapter and libx264; browser compatibility and physical GPU encoder performance require separate validation.
