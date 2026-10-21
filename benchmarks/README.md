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
