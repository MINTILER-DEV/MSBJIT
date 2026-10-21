"""Compare two MSBJIT executables with identical headless turbo workloads."""
import argparse
import json
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


def workload(kind):
    blocks = {
        "flag": {"opcode": "event_whenflagclicked", "topLevel": True, "next": "loop"},
        "loop": {"opcode": "control_forever", "inputs": {"SUBSTACK": [2, "work"]}},
        "inc": {"opcode": "data_changevariableby", "fields": {"VARIABLE": ["calls", "n"]},
                "inputs": {"VALUE": [1, [4, "1"]]}},
    }
    lists = {}
    if kind.startswith("broadcast"):
        blocks["work"] = {"opcode": "event_broadcastandwait",
                          "inputs": {"BROADCAST_INPUT": [1, [11, "ping", "message"]]}}
        blocks["receiver"] = {"opcode": "event_whenbroadcastreceived", "topLevel": True,
                              "fields": {"BROADCAST_OPTION": ["ping", "message"]}, "next": "inc"}
        if kind.endswith("ram"):
            lists = {"ram": ["RAM", [0] * 65536], "hdd": ["HDD", [0] * 65536]}
    elif kind == "numeric-list":
        lists = {"ram": ["RAM", [0] * 65536]}
        blocks["work"] = {"opcode": "data_replaceitemoflist", "fields": {"LIST": ["RAM", "ram"]},
                          "inputs": {"INDEX": [1, [4, "32768"]], "ITEM": [2, "plus"]}, "next": "read"}
        blocks["plus"] = {"opcode": "operator_add", "inputs": {
            "NUM1": [3, [12, "calls", "n"], [4, "0"]], "NUM2": [1, [4, "1"]]}}
        blocks["read"] = {"opcode": "data_setvariableto", "fields": {"VARIABLE": ["calls", "n"]},
                          "inputs": {"VALUE": [2, "item"]}}
        blocks["item"] = {"opcode": "data_itemoflist", "fields": {"LIST": ["RAM", "ram"]},
                          "inputs": {"INDEX": [1, [4, "32768"]]}}
    else:
        blocks["work"] = blocks.pop("inc")
    return {"targets": [{"name": "Stage", "isStage": True,
                         "variables": {"n": ["calls", 0]}, "lists": lists, "blocks": blocks}]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--seconds", type=float, default=2)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--project", type=Path, help="Also measure the external hardware emulator (CLK)")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.seconds <= 0 or args.repeats < 1:
        parser.error("seconds and repeats must be positive")
    results = {"platform": platform.platform(), "seconds": args.seconds, "repeats": args.repeats,
               "executables": {k: str(getattr(args, k).resolve()) for k in ("before", "after")},
               "workloads": {}}
    with tempfile.TemporaryDirectory(prefix="msbjit-bench-") as temp:
        projects = {}
        for name in ("arithmetic", "numeric-list", "broadcast-empty", "broadcast-ram"):
            path = Path(temp) / (name + ".json")
            path.write_text(json.dumps(workload(name)), encoding="utf-8")
            projects[name] = path
        if args.project:
            projects["hardware-emulator"] = args.project.resolve()
        for name, path in projects.items():
            samples = {"before": [], "after": []}
            for repeat in range(args.repeats):
                # Alternate order to reduce warm-up/order bias.
                for version in (("before", "after") if repeat % 2 == 0 else ("after", "before")):
                    start = time.perf_counter()
                    process = subprocess.run([results["executables"][version], str(path), "--headless",
                                              "--turbo", "--seconds", str(args.seconds), "--json"],
                                             capture_output=True, text=True, check=True,
                                             timeout=max(120, args.seconds + 60))
                    wall = time.perf_counter() - start
                    report = json.loads(process.stdout)
                    counter = "CLK" if name == "hardware-emulator" else "calls"
                    count = next(v["value"] for v in report["variables"] if v["name"] == counter)
                    samples[version].append({"count": count, "steps": report["steps"],
                                             "elapsed": report["elapsed_seconds"], "wall": wall,
                                             "count_per_second": count / report["elapsed_seconds"]})
            rates = {v: statistics.median(s["count_per_second"] for s in runs)
                     for v, runs in samples.items()}
            speedup = rates["after"] / rates["before"] if rates["before"] else None
            results["workloads"][name] = {"samples": samples, "rates": rates, "speedup": speedup}
            speedup_text = f"{speedup:.2f}x" if speedup is not None else "baseline counter is zero"
            print(f"{name}: {rates['before']:.1f} -> {rates['after']:.1f} /s ({speedup_text})", flush=True)
            args.output.write_text(json.dumps(results, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
