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

The Scratch username reporter returns `Player1` by default. Override it with
`--username "YourName"`, for example `msbjit game.sb3 --turbo --username "Alex"`.
Library callers can set `RuntimeOptions.username`.

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

Repeat and foreach counters use native loads/stores in the active procedure frame. Custom-block calls jump directly to their compiled entry after setting up the frame; returns and scheduler resumption restore its argument and loop buffers. Numeric coercions of immutable literals are computed during compilation without changing their string behavior in other contexts.

Values exposed to Rust use `ScratchValue::{Number, String, Boolean}`. The JIT ABI uses NaN-boxed values with unboxed floating-point numbers, interned string handles, and boolean tags. A collector reclaims temporary strings at scheduler boundaries, preserving constants, variables, lists, and procedure arguments. Lists track their string roots incrementally, so numeric RAM/disk buffers do not need a full scan. Collection is skipped when no new strings were allocated.

Interned strings cache numeric conversions, and comparisons borrow their text instead of cloning it. ASCII comparisons avoid allocating lowercase copies. Numeric text literals can compare natively against unboxed numbers; whitespace, nonnumeric strings, and dynamically boxed operands retain the Scratch coercion fallback.

Arithmetic reads cached conversions for the first 65,536 string handles directly from a fixed 512 KiB table; larger handles use the helper fallback. Reclaimed handles refresh their cache entries when reused. Numeric string and boolean list indices use native range checks, while special indices such as `last`, `random`, and `any` retain their text and helper behavior. The original string values remain intact for joins, comparisons, and list contents.

Each cooperative script retains a program counter, procedure frames, arguments, and repeat counters. Normal loop backedges yield until the next tick. Straight-line code and turbo loops cooperate after a 2 ms slice. Warp custom blocks and their callees skip ordinary yields, while checkpoints enforce cancellation, step/time limits, recursion limits, and the warp watchdog. A warp budget expiration yields cooperatively and resumes with its procedure stack intact. Explicit waits and broadcast-and-wait suspend scripts, including warp scripts. Broadcast receivers restart when their message is received again.

Custom blocks are compiled when reachable from an event script or another reachable custom block. Unused definitions can contain unsupported blocks without preventing execution. Surplus defaults left over after parameter edits are ignored; calls with missing definitions do nothing, and missing argument reporters return zero, matching Scratch's procedure behavior. Inside a custom block, ?stop this script? returns from that custom block; at the event-script root it stops the script.

Native code counts every IR instruction; clock/cancellation checks are sampled every 64 instructions and on scheduler entry or warp-mode transitions. Step limits remain exact. Expensive individual helpers can extend the interval between clock checks. Turbo broadcast waiters resume without an artificial sleep once their receivers complete; ordinary loop frame pacing and explicit waits still apply.

The renderer reads atomic sprite components without locking the execution thread. Native and streaming displays batch pen strokes into instanced GPU capsule draws on a persistent 480 by 360 layer between the backdrop and sprites. The VM publishes pen work after a scheduler round, including broadcast receivers; host checkpoints inside warp blocks keep the previous completed pen image visible. Explicit waits allow presentation. Long uninterrupted warps may present after 500 ms or at the 8 MiB command limit to bound latency and memory. Slow pen consumers apply backpressure, and a new clear discards obsolete queued frames. Costume stamps retain CPU effect/sampling and upload an overlay. Headless execution retains the CPU pen canvas. Sprite snapshots can span multiple updates; this is not a transactionally consistent whole-project snapshot. GPU rendering/capture occurs on the calling main thread. Bounded video queues drop frames when encoding/network delivery falls behind. FFmpeg input/output have dedicated threads; signaling and WebRTC use Tokio workers.

## Supported blocks

- Green flag, broadcast-received, and clone-start hats; broadcast and broadcast-and-wait dispatch to originals and live clones. Create clones of self or named sprites and delete clones, with a 300-live-clone limit. Clones snapshot sprite-local variables, lists, appearance, effects, and pen settings; stage data remains shared. Deletion stops every script on that clone; green flag and stop clear clones.
- Key-pressed hats (letters, digits, arrows, space, Enter, and any), sprite/stage clicks, key-pressed and mouse x/y/down reporters. Clicks select the frontmost visible, nontransparent costume pixel with size, rotation, mirroring, and layer order applied.
- Variable set, change, show/hide, and reporters; sprite-local and stage variables. Saved variable monitors appear above the stage in normal, large, and slider modes, with Scratch-style colors, typography, positions, labels, and six-decimal numeric readouts. Sliders update the actual running variable, respect saved ranges and integer/0.01 steps, and capture dragging outside the panel. Drag a monitor header to move it; double-click to cycle normal, large, and slider displays. Both the native window and browser stream include the same monitors. List monitors remain unsupported.
- List add, delete, delete all, insert, replace, item, item number, length, contains, and contents reporters; list IDs resolve locally with stage fallback. Lists preserve strings across scheduler collection and use Scratch's 200,000-item mutation limit and special indices.
- Repeat, foreach, forever, repeat-until, while, wait-until, if, if/else, wait, and all three stop modes. Foreach keeps its own loop index, rechecks its bound, and applies normal frame pacing outside warp/turbo.
- Custom block definitions/calls with parameters, defaults, recursion, and inherited warp mode.
- Arithmetic, modulo, comparisons, booleans, random, join, contains, letter, length, round, and standard math reporters. Translate's viewer-language reporter returns `English` by default; `--language "Fran?ais"` selects another language name. The translation command itself is not implemented.
- Move steps, go to x/y or another sprite/mouse/random position, timed glide to x/y or a sampled target position, set/change x/y, point direction or towards a target/mouse, turn, and rotation style. Glides interpolate in real time in both normal and turbo modes and draw pen trails. Front/back and relative layer changes affect drawing and click order, including clones. Attribute-of-object reporters read sprite/stage position, direction, size, costume/backdrop, volume, and variables.
- Ask-and-wait queues questions without blocking other scripts; the answer reporter preserves entered text. The native stage supports typing, Backspace, Enter, and a submit button; the browser player provides a text form. Green flag clears the answer, and canceled scripts discard their prompts. Headless hosts can send `InputEvent::Answer { id, text }`; unanswered questions wait for input or the execution limit. Distance-to supports sprites and mouse position.
- Show/hide, set/change size, switch/next costume, costume reporters, say/think logging, timer/reset timer, and days since 2000.
- Set/change color, fisheye, whirl, pixelate, mosaic, brightness, and ghost effects; clear graphic effects. Effects apply to sprites, backdrops, stamps, and click picking. Brightness and ghost clamp in block units; the other effects retain their raw values and use Scratch's renderer conversions. Green flag, player stop, and stop-all clear all effects; finishing one script preserves them. Fully ghosted sprites do not receive clicks; edge sensing ignores ghost.
- Pen clear, up/down, size, RGB color, hue/saturation/brightness/transparency parameters, and parameter/size changes. Round strokes persist across frames and green flags; erase-all clears the shared layer. Pen rasterization uses CPU coverage and can differ slightly at stroke edges from Scratch's GPU renderer.
- SVG, PNG, and JPEG costumes, bitmap resolution, rotation centers, sprite rotation/visibility/size, and initial layer order. The stage is 480 × 360.
- Mouse and sprite/clone touching use transformed costume alpha; sprite collisions exclude hidden sprites and ignore ghost, while mouse sensing also works for hidden sprites. Persistent costume stamps and touching-stage-edge sensing, including costume alpha bounds, rotation centers, scaling, and mirroring. Stamps use premultiplied alpha with bitmap nearest sampling and SVG linear sampling when distorted, rotated, or scaled. The costume cache is bounded to 64 MiB. These blocks require the graphics feature, which is enabled by default and works headlessly.
- Start-sound and play-until-done for mono/stereo PCM or floating-point WAV assets and MP3 assets. The Windows host mixes simultaneous sounds and resumes waiting scripts on playback completion; headless and non-Windows runs preserve sound duration without audio output. Stop-all-sounds releases waiters, while missing sound references return immediately. Volume, pitch, and stereo pan controls apply to playing sounds; clones inherit settings. Replaying the same sprite sound replaces that sound, and deleting a clone stops its sounds. Browser streaming sends video only; native Linux audio output and compressed WAV formats remain unsupported.

Cloud-variable declarations load with their saved names and values and run locally. Scratch cloud networking is not implemented.

This is a bounded Scratch subset. Other extensions, backdrop commands, list monitors, and speech-bubble rendering are not implemented. Say/think text appears in the final report. SVG text uses bundled Arimo fallback fonts and Scratch's Knewave font for Marker text, including on headless hosts. Other Scratch font families currently use the fallback. Raw JSON projects support execution; costume rendering requires assets from an `.sb3` archive. String operators currently use Unicode scalar characters rather than JavaScript UTF-16 code units, so non-BMP text may differ from Scratch. Size/position clamping also differs from Scratch's renderer. Key hats fire on a new press and do not overlap an already-running instance of the same hat; OS key auto-repeat is not forwarded. Green flag cancels current scripts and resets the timer; it preserves changed variables and sprite properties, as Scratch scripts are responsible for initializing them.

Graphic effects are checked against 23 independent reference images produced by Scratch's GLSL shader, including combined effects, grayscale, transparency, negative values, bitmap resolution, and SVG filtering. See [effect reference fixtures](tests/fixtures/effects/README.md) for sources, regeneration, and GPU validation. Comparisons allow two 8-bit levels per channel for floating-point and framebuffer rounding; original costume rasterization and SVG scaling can still differ from Scratch.

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

To validate the external RISC-V Linux project, set `MSBJIT_LINUX_PROJECT` and run `cargo test --release --all-features --lib riscv_linux_project -- --ignored --nocapture`. This checks CPU progress and terminal drawing; set `MSBJIT_LINUX_CAPTURE` to a PNG path to save the stage. Use an optimized build for this large emulator; long warp slices yield and resume during RAM initialization.

To validate the external ray tracer, set `MSBJIT_RAYTRACE_PROJECT` to `raytrace.sb3` and run `cargo test --all-features --lib raytrace_intro_rendering_sliders_and_keyboard -- --ignored --nocapture`. This runs the original clone-based introduction, completes a frame using the project's resolution slider, moves the camera with arrow input, and checks pen output. Set `MSBJIT_RAYTRACE_INTRO_CAPTURE` and `MSBJIT_RAYTRACE_CAPTURE` to PNG paths to save both stages.

Compare optimized executables with `python tools/benchmark.py BEFORE.exe AFTER.exe --output results.json`. This alternates their order and reports median progress for arithmetic, custom-block loops, numeric lists, and broadcasts. To measure the original ray tracer separately, add `--project raytrace.sb3 --project-counter steps --project-only --seconds 20`. Its instruction throughput includes the introduction and CPU pen rendering; it is not a displayed frame-rate measurement. Use the same project and settings for both executables and run benchmarks without other builds running.

For ray-tracing speed, use `python tools/benchmark_raytrace.py BEFORE.exe AFTER.exe raytrace.sb3 --output frames.json`. It saves a separate benchmark fixture that starts the existing renderer directly, disables the busy-wait introduction, and renders exactly one frame. Rendering procedures, assets, resolution, shadows, and quality settings are preserved. It alternates executables and compares median time to finish the frame; the original `.sb3` is untouched. This measures headless computation, not native-window or stream FPS.

On Windows with an Intel i5-8300H, the full-resolution ray tracer improved from **20.67 s to 4.04 s per completed frame (5.11×)** between `0f47f98` and `ab03297`. These are medians of three alternating optimized runs with resolution 1, quality 1, and shadows enabled. See the [recorded samples and method](benchmarks/raytrace-2026-10-22.json). Procedural noise varies per run; timings include the original 0.25-second startup wait and exclude JIT compilation.

For runtime-helper call counts, set `MSBJIT_PROFILE_HELPERS=1` before running a bounded project. Counts print to stderr when execution ends; they are frequencies, not sampled CPU timings. Unset this variable for performance measurements: ordinary JIT code contains no profiling counter instructions.

Set `MSBJIT_DOIRS_PROJECT` to `doirsout.sb3` and run `cargo test --all-features --lib doirs_project_renders_and_starts_without_runtime_errors -- --ignored --nocapture` to check loading, menu rendering, and the Space start input. `MSBJIT_DOIRS_MENU_CAPTURE` and `MSBJIT_DOIRS_CAPTURE` save PNGs. The current SBText export has no sound assets, renamed costume metadata, and missing input/motion reporters replaced with zero; these affect appearance and block movement independently of MSBJIT. Movement and collision regressions use separate synthetic projects.

For a regenerated Doirs export, also set `MSBJIT_DOIRS_RESTORED=1` to assert that arrow keys move the player. To verify every sound asset, set `MSBJIT_AUDIO_PROJECT` to the archive and run `cargo test --all-features --lib all_external_project_sounds_decode -- --ignored --nocapture`.
