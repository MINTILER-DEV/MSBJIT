# MSBJIT

A Rust runtime for Windows and Linux that loads Scratch 3 `.sb3` projects, lowers scripts to a validated control-flow graph, and compiles that graph to native code with Cranelift. Execution runs on a dedicated thread. An interactive native stage window and optional local WebRTC streaming run independently of the script tick rate.

Requires Rust 1.93 or newer. The default build includes a native stage window using wgpu and minifb. It needs a graphics adapter supported by wgpu; **the local window does not need FFmpeg**. The optional streaming feature also needs an `ffmpeg` executable with an H.264 encoder on PATH.

## Windows quick start

Install Rust with the MSVC toolchain and Visual Studio Build Tools with the **Desktop development with C++** workload. Run these commands in PowerShell from the repository:

```powershell
cargo build --release --locked
.\target\release\msbjit.exe .\examples\interactive.sb3
# Or open your own project (paths with spaces must be quoted):
.\target\release\msbjit.exe "C:\Projects\My game.sb3"
```

The native stage opens automatically for `.sb3` files and starts the green-flag scripts once the renderer is ready. Click its green flag to start them again, or the red square to stop scripts while keeping the window open. Closing the window exits the host. The stage keeps its 4:3 shape when resized, and mouse input accounts for scaling and letterboxing. Keyboard input goes to the focused window; losing focus releases held keys and mouse buttons.

Windows uses Direct3D 12 by default, avoiding enumeration of optional Vulkan drivers. If needed, select a different wgpu backend explicitly, for example `$env:WGPU_BACKEND = "gl"` in PowerShell for OpenGL, or `"vulkan"` for Vulkan.

In `interactive.sb3`, arrow keys move the sprite, clicking its visible pixels changes its costume, and space moves it to the mouse. The green flag returns it to the center. Rebuild this example with `python examples/make_interactive.py`.

## Running projects

```sh
cargo run --release -- examples/counter.json --turbo --json
cargo run --release -- examples/demo.sb3 --seconds 5
cargo run --release -- examples/demo.sb3 --headless --seconds 5 --json
cargo run --release --features streaming -- examples/demo.sb3 \
  --serve 127.0.0.1:8080 --fps 144 --encoder auto
```

For streaming, open `http://127.0.0.1:8080`, click **Connect**, then **Green flag**. Click the stage to focus it for keyboard and pointer input. **Stop** cancels current scripts; **Green flag** starts them again. Browser clients share control of one project; disconnecting releases held inputs. Press Ctrl-C to stop the host. To view from another machine on a trusted LAN, bind to `0.0.0.0:8080` and open the host's LAN address. The player uses host ICE candidates with an empty ICE server list; there are no STUN/TURN or third-party signaling services. This server has no authentication and is intended for loopback or a trusted private network.

Windows streaming command (requires FFmpeg):

```powershell
cargo run --release --features streaming -- .\examples\interactive.sb3 --serve 127.0.0.1:8080 --encoder auto
```

Use `--display` to request a window explicitly, including for asset-free JSON projects. Use `--headless` for batch runs, or `cargo run --no-default-features -- <project>` to build without window/graphics dependencies. `--serve` selects the browser player instead of the local window. On Linux, the default minifb build needs X11/Wayland development libraries (`libx11-dev libxkbcommon-dev libwayland-dev` on Debian/Ubuntu).

`--seconds` sets an execution limit and, when streaming, a host duration. `--max-steps` caps executed IR instructions. `--tick-hz` defaults to 30. `--turbo` removes loop frame boundaries without removing explicit waits. `--watchdog-ms` may lower the warp execution budget from its maximum of 500 ms. `--json` prints final variables, sprite state, captured say/think messages, and execution statistics. Compilation time is excluded from execution statistics.

The loader accepts up to 256 MiB of decompressed project JSON by default, for both `.sb3` archives and standalone `.json` files. Compressed archive size can be much smaller than its JSON. For larger projects, pass `--max-project-mib 512` (1–4096 MiB supported). This changes the JSON limit; costume asset limits remain separate. Large list data also requires memory for its parsed and runtime representations.

Set `MSBJIT_LARGE_PROJECT` to an external large `.sb3` or `.json` path to include it in the opt-in loader regression (`cargo test --lib external_large_project_loads -- --ignored`). This checks loading; block compilation and project execution have their own compatibility requirements. The external fixture is skipped when its path is not supplied.

## Execution model

The loader reads bounded ZIP entries directly without extracting them. Variables resolve by Scratch ID with stage fallback. Block graphs are checked for missing links, cycles, and excessive depth. Active unsupported commands/reporters and event hats produce errors with block context; detached workspace stacks are ignored.

Cranelift emits native branches, loop boundaries, variable/argument loads and stores, numeric list access, arithmetic, numeric comparisons, boolean operations, and common math operations. Dynamic string coercions, special list indices, string-list mutations, event dispatch, and sprite changes use Rust helpers. Procedure frames and argument buffers are reused between calls. This is a JIT, but dynamic helper calls still have overhead; it does not promise C-equivalent throughput for every Scratch workload.

Values exposed to Rust use `ScratchValue::{Number, String, Boolean}`. The JIT ABI uses NaN-boxed values with unboxed floating-point numbers, interned string handles, and boolean tags. A collector reclaims temporary strings at scheduler boundaries, preserving constants, variables, lists, and procedure arguments. Lists track their string roots incrementally, so numeric RAM/disk buffers do not need a full scan. Collection is skipped when no new strings were allocated.

Each cooperative script retains a program counter, procedure frames, arguments, and repeat counters. Normal loop backedges yield until the next tick. Straight-line code and turbo loops cooperate after a 2 ms slice. Warp custom blocks and their callees skip ordinary yields, while checkpoints enforce cancellation, step/time limits, recursion limits, and the warp watchdog. A watchdog expiration terminates the run with an error. Explicit waits and broadcast-and-wait suspend scripts, including warp scripts. Broadcast receivers restart when their message is received again.

Custom blocks are compiled when reachable from an event script or another reachable custom block. Unused definitions can contain unsupported blocks without preventing execution. Surplus defaults left over after parameter edits are ignored; calls with missing definitions do nothing, and missing argument reporters return zero, matching Scratch's procedure behavior. Inside a custom block, ?stop this script? returns from that custom block; at the event-script root it stops the script.

Native code counts every IR instruction; clock/cancellation checks are sampled every 64 instructions and on scheduler entry or warp-mode transitions. Step limits remain exact. Expensive individual helpers can extend the interval between clock checks. Turbo broadcast waiters resume without an artificial sleep once their receivers complete; ordinary loop frame pacing and explicit waits still apply.

The renderer reads atomic sprite components without locking the execution thread. Pen drawing uses a bounded 480 by 360 CPU canvas protected by a short mutex; the renderer copies changed canvas generations and uploads them between the backdrop and sprites. A snapshot can span multiple state updates; it is not a transactionally consistent whole-project snapshot. GPU rendering/capture occurs on the calling main thread. Bounded queues drop frames when encoding/network delivery falls behind, rather than blocking scripts. FFmpeg input/output have dedicated threads; signaling and WebRTC use Tokio workers.

## Supported blocks

- Green flag and broadcast-received hats; broadcast and broadcast-and-wait.
- Key-pressed hats (letters, digits, arrows, space, Enter, and any), sprite/stage clicks, key-pressed and mouse x/y/down reporters. Clicks select the frontmost visible, nontransparent costume pixel with size, rotation, mirroring, and layer order applied.
- Variable set, change, and reporters; sprite-local and stage variables.
- List add, delete, delete all, insert, replace, item, item number, length, contains, and contents reporters; list IDs resolve locally with stage fallback. Lists preserve strings across scheduler collection and use Scratch's 200,000-item mutation limit and special indices.
- Repeat, foreach, forever, repeat-until, while, wait-until, if, if/else, wait, and all three stop modes. Foreach keeps its own loop index, rechecks its bound, and applies normal frame pacing outside warp/turbo.
- Custom block definitions/calls with parameters, defaults, recursion, and inherited warp mode.
- Arithmetic, modulo, comparisons, booleans, random, join, contains, letter, length, round, and standard math reporters.
- Move steps, go to x/y, set/change x/y, point direction, turn, and rotation style.
- Show/hide, set/change size, switch/next costume, costume reporters, say/think logging, timer/reset timer, and days since 2000.
- Pen clear, up/down, size, RGB color, hue/saturation/brightness/transparency parameters, and parameter/size changes. Round strokes persist across frames and green flags; erase-all clears the shared layer. Pen rasterization uses CPU coverage and can differ slightly at stroke edges from Scratch's GPU renderer.
- SVG, PNG, and JPEG costumes, bitmap resolution, rotation centers, sprite rotation/visibility/size, and initial layer order. The stage is 480 × 360.
- Persistent costume stamps and touching-stage-edge sensing, including costume alpha bounds, rotation centers, scaling, and mirroring. Stamps use premultiplied alpha and nearest-pixel sampling; their edges can differ from GPU-filtered Scratch stamps. The costume cache is bounded to 64 MiB. These blocks require the graphics feature, which is enabled by default and works headlessly.
- Start-sound playback for WAV assets in the Windows native host. Playback is asynchronous; the current sound is kept alive until completion/replacement/stop. This basic output supports one voice, and a new sound replaces the previous one. Headless runs omit audio output; browser streaming currently sends video only. Native audio output on Linux, compressed sound formats, and other sound controls remain unsupported.

This is a bounded Scratch subset. Clones, sprite-to-sprite collisions, ask/answer, other extensions, glide, effects, backdrop commands, variable/list monitors, and speech-bubble rendering are not implemented. Say/think text appears in the final report. SVG text/font support is disabled in this build. Raw JSON projects support execution; costume rendering requires assets from an `.sb3` archive. String operators currently use Unicode scalar characters rather than JavaScript UTF-16 code units, so non-BMP text may differ from Scratch. Size/position clamping also differs from Scratch's renderer. Key hats fire on a new press and do not overlap an already-running instance of the same hat; OS key auto-repeat is not forwarded. Green flag cancels current scripts and resets the timer; it preserves changed variables and sprite properties, as Scratch scripts are responsible for initializing them.

## Streaming and performance limits

`--encoder auto` probes NVENC, then software libx264 on Windows, and NVENC, VA-API, then libx264 on Linux, using an actual one-frame encode. Explicit choices are `nvenc`, `vaapi` (Linux only), and `software`; `--vaapi-device` selects the Linux render device. Every encoder disables B-frames, uses a one-second keyframe interval, and produces H.264 Annex-B access units. webrtc-rs packetizes those units as RTP/SRTP over UDP. The bundled browser forwards input and player controls over the existing local WebSocket.

**GPU capture currently uses portable texture-to-buffer readback followed by a CPU pipe to FFmpeg.** DMA-BUF/CUDA/VA-API texture interop and a zero-copy encoder path remain unimplemented. AV1 is not implemented. The spec's sub-millisecond capture, 2–5 ms hardware encoding, and 5–25 ms network budgets are performance targets, not measured guarantees. A 144 Hz capture setting does not guarantee that the available GPU, encoder, network, or browser will sustain that rate.

## Verification

```sh
cargo test
cargo test --all-features
cargo test --no-default-features
# Optional GPU/FFmpeg/WebRTC integration checks:
cargo test --all-features -- --ignored
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
# Browser input contract tests (Node.js 24):
node --test tests/player.cjs
```

Runtime tests execute generated native code for arithmetic/coercion, branches, nested loops, frame pacing, warp inheritance/watchdogs, cancellation, explicit waits, procedure arguments, broadcasts, string lifetime, and atomic scene publication. Streaming unit tests cover Annex-B parsing. Opt-in integration tests validate captured pixel colors and GPU → FFmpeg → WebSocket/host ICE → H.264 RTP delivery to a native WebRTC peer. These can run through a software wgpu adapter and libx264; browser compatibility and physical GPU encoder performance require separate validation.

Input tests also execute native key/click hats and sensing reporters, idle event dispatch, focus-loss release, stop/green-flag behavior, and window pointer-coordinate mapping. CI runs compilation, tests, formatting, and Clippy on Windows and Linux; GUI/GPU/FFmpeg integration checks remain opt-in.

To include the external SBText-RS emulator regression in the opt-in checks, set `MSBJIT_HARDWARE_PROJECT` to the path of `hardware_emu.sb3`. It checks RAM/disk initialization, CPU terminal output, pen pixels, and the Up Arrow step-mode control. Set `MSBJIT_HARDWARE_CAPTURE` to a `.png` path to save its rendered stage. The fixture is skipped when its path is not supplied.

To validate the external RISC-V Linux project, set `MSBJIT_LINUX_PROJECT` and run `cargo test --release --all-features --lib riscv_linux_project -- --ignored --nocapture`. This checks CPU progress and terminal drawing; set `MSBJIT_LINUX_CAPTURE` to a PNG path to save the stage. Use an optimized build for this large emulator so RAM initialization stays within the warp watchdog.
