# Windows runtime measurements

Measured 2026-10-21 on Windows 11, Intel Core i5-8300H. Both executables were
built with `cargo build --release --all-features --locked --jobs 2`, using the
repository's thin LTO release profile. Baseline: `b015e59`. Optimized runtime:
`2e1e49e` (subsequent test-only changes do not change runtime behavior).

Each workload ran headless with `--turbo --seconds 2`, three times per executable,
alternating executable order. Rates below are the median of completed workload
iterations divided by execution time. Compilation, loading, and final JSON
serialization are excluded from execution time; process wall times are included
in the raw results. These are local measurements, not performance guarantees.

| Workload | Before / second | After / second | Speedup |
| --- | ---: | ---: | ---: |
| Arithmetic increment loop | 4,977,451 | 135,908,617 | 27.3x |
| Numeric list read/replace | 3,280,348 | 60,167,285 | 18.3x |
| Broadcast-and-wait RPC | 666 | 1,125,328 | 1,689x |
| Same RPC with two 65,536-item numeric lists | 610 | 1,202,844 | 1,973x |
| SBText-RS hardware emulator (`CLK`) | 1,757 | 125,336 | 71.3x |

The very large broadcast gains mostly come from eliminating an unintended 1 ms
sleep after a receiver had already completed. They do not indicate a similar
speedup for all Scratch blocks. Normal mode still preserves 30 Hz loop boundaries;
use `--turbo` when comparing unrestricted execution throughput.

The hardware-emulator rate includes its RAM/disk initialization and boot during
the two-second execution interval. It is not an isolated CPU instruction benchmark.
Median total process wall time was 2.40 s before and 2.61 s after: the expanded
native fast paths increase JIT compilation work. Faster execution does not imply
faster startup. The fixture is external and not redistributed here:
`SBText-RS/testprojects/hardware_emu.sb3`, SHA-256
`523d313f7fc6dab1f4efa0b2fc755e228cdda7a4cea072da546b1992a0768912`.

The emulator also passed a separate GPU capture/keyboard regression: initialized
RAM/disk, CPU terminal output, visible pen pixels, and Up Arrow step-mode input.
No TurboWarp benchmark was performed; these results compare MSBJIT revisions only.

To reproduce with two separately built executables:

```powershell
python tools/benchmark.py path/to/before.exe path/to/after.exe `
  --seconds 2 --repeats 3 `
  --project D:/GitHub/Repositories/SBText-RS/testprojects/hardware_emu.sb3 `
  --output results.json
```

Omit `--project` to run only the four self-contained generated workloads.
See [windows-2026-10-21.json](windows-2026-10-21.json) for all release samples.

## sub1k pen rendering, 2026-10-22

The pen renderer previously rasterized every stroke on the CPU and allowed
the display to read a canvas while a script erased and rebuilt it. Native and
streaming displays now draw instanced GPU capsules and publish pen work at
scheduler boundaries. Short host interruptions inside warp scripts retain the
previous pen image. Numeric string coercions and numeric string list indices
also have native fast paths in runtime revision `9e222c3`.

On the same i5-8300H and a GTX 1050 Ti using DX12, three alternating samples
per backend each completed **30 render calls**, including scene initialization:

| Camera pitch | CPU pen, median frames/s | GPU pen, median frames/s | Speedup |
| --- | ---: | ---: | ---: |
| Initial view, 0 degrees | 4.60 | 23.21 | 5.05x |
| Steep view, 103.3 degrees | 4.33 | 23.83 | 5.51x |

These isolate CPU versus GPU pen drawing within the same optimized runtime.
They include GPU stage capture/readback but omit the native window blit and
video encoding. Loading and JIT compilation are excluded; project initialization
and rendering quality are retained. Background applications were left running:
steep-view GPU samples ranged from 14.66 to 27.82 frames/s, so these short runs
do not establish stable 30 FPS or TurboWarp parity. Final stage images matched
the CPU reference with mean absolute channel error below one 8-bit level;
variable-monitor text was excluded from the image comparison.

The installed native player also completed a separate **357 frames in 10 seconds
(35.70 frames/s)** at 103.3 degrees, with the same quality settings and an explicit
completion counter. Its final project FPS reporter read 38.37. A ten-second run
of the unchanged original project advanced its frame-start counter by 347 and
ended with an FPS reading of 33.31; that counter increments before drawing and
is not an exact count of completed frames. These are separate smoke runs, not
medians or guarantees of sustained frame rate. The longer runs amortize project
initialization differently from the 30-frame samples.

See [all samples and native smoke results](sub1k-2026-10-22.json). The external
project and its assets are not redistributed. To reproduce:

```powershell
python tools/benchmark_sub1k.py D:/Downloads/sub1k.sb3 local/sub1k-30.sb3 --rot-x 103.3
$env:MSBJIT_PEN_BENCH_PROJECT = (Resolve-Path local/sub1k-30.sb3).Path
$env:MSBJIT_PEN_BENCH_OUTPUT = "local/sub1k-results.json"
cargo test --release --lib sub1k_complete_frames -- --ignored --nocapture
```

Omit `--rot-x` for the original initialized view. The generated fixture changes
the main forever loop to repeat 30, waits for the render broadcast, and moves
the frame-counter increment after the render call. This prevents a restarted,
unfinished warp script from being counted as a completed frame. Rendering
procedures, resolution, lighting, shadows, assets, and other quality settings
are unchanged. The original archive is untouched.
