"""Compare complete ray-traced frames, excluding the project's busy-wait intro.

The saved fixture starts the existing 'start' receiver with the green flag,
disables Intro scripts, and replaces the outer forever with repeat 1. Rendering
procedures, assets, resolution, shadows, and quality settings are unchanged.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess
import time
import zipfile


def one_frame(source, destination):
    with zipfile.ZipFile(source) as archive:
        project = json.loads(archive.read("project.json"))
        pen = next(t for t in project["targets"] if t["name"] == "pen")
        intro = next(t for t in project["targets"] if t["name"] == "Intro")
        intro["blocks"] = {}
        intro["visible"] = False
        blocks = pen["blocks"]
        hats = [b for b in blocks.values() if b["opcode"] == "event_whenbroadcastreceived"
                and b["fields"]["BROADCAST_OPTION"][0] == "start"]
        if len(hats) != 1:
            raise ValueError("expected exactly one pen/start receiver")
        hat = hats[0]
        hat["opcode"] = "event_whenflagclicked"
        hat["fields"] = {}
        cursor, visited = hat, set()
        while cursor["opcode"] != "control_forever":
            key = cursor.get("next")
            if key is None or key in visited:
                raise ValueError("expected a main render forever loop")
            visited.add(key)
            cursor = blocks[key]
        cursor["opcode"] = "control_repeat"
        cursor["inputs"]["TIMES"] = [1, [4, "1"]]
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as output:
            for entry in archive.infolist():
                output.writestr(entry, json.dumps(project).encode() if entry.filename == "project.json"
                                else archive.read(entry))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("project", type=Path)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    fixture = args.output.with_suffix(".one-frame.sb3")
    inputs = {p.resolve() for p in (args.before, args.after, args.project)}
    if args.repeats < 1 or fixture.resolve() in inputs or args.output.resolve() in inputs:
        parser.error("use a positive repeat count and separate output paths")
    one_frame(args.project, fixture)
    results = {"platform": platform.platform(), "source_sha256": hashlib.sha256(args.project.read_bytes()).hexdigest(),
               "fixture": str(fixture.resolve()), "description": __doc__,
               "executables": {v: str(getattr(args, v).resolve()) for v in ("before", "after")},
               "samples": {"before": [], "after": []}}
    for repeat in range(args.repeats):
        for version in (("before", "after") if repeat % 2 == 0 else ("after", "before")):
            start = time.perf_counter()
            run = subprocess.run([str(getattr(args, version).resolve()), str(fixture.resolve()),
                                  "--headless", "--turbo", "--json"], capture_output=True,
                                 text=True, check=True, timeout=180)
            report = json.loads(run.stdout)
            frames = float(next(v["value"] for v in report["variables"] if v["name"] == "time"))
            if report["interrupted"] or frames != 1:
                raise ValueError(f"{version} failed to complete exactly one frame")
            sample = {"elapsed": report["elapsed_seconds"], "wall": time.perf_counter() - start,
                      "steps": report["steps"], "frames": frames}
            results["samples"][version].append(sample)
            print(f"{version} #{repeat + 1}: {sample['elapsed']:.3f}s, {sample['steps']} steps", flush=True)
            args.output.write_text(json.dumps(results, indent=2), encoding="utf-8")
    results["median_seconds"] = {v: statistics.median(s["elapsed"] for s in samples)
                                 for v, samples in results["samples"].items()}
    results["speedup"] = results["median_seconds"]["before"] / results["median_seconds"]["after"]
    args.output.write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(f"Single-frame speedup: {results['speedup']:.2f}x", flush=True)


if __name__ == "__main__":
    main()
