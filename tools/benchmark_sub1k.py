"""Make a finite sub1k render benchmark without changing rendering quality.

Only the MAIN ENGINE's outer forever becomes repeat N. No input is supplied,
so the camera stays at its initialized position. All broadcast receivers must
finish before MSBJIT reports completion; this measures whole frames rather
than the project's instantaneous FPS variable. No project assets are copied
into the repository. Run the ignored sub1k_complete_frames benchmark with
MSBJIT_PEN_BENCH_PROJECT pointing at the resulting archive.
"""
import argparse
import json
from pathlib import Path
import zipfile


def fixture(source, destination, frames):
    if frames < 1 or source.resolve() == destination.resolve():
        raise ValueError("use positive frames and a separate output path")
    with zipfile.ZipFile(source) as archive:
        project = json.loads(archive.read("project.json"))
        engine = next(t for t in project["targets"] if t["name"] == "MAIN ENGINE")
        loops = [b for b in engine["blocks"].values() if b["opcode"] == "control_forever"]
        if len(loops) != 1:
            raise ValueError("expected one MAIN ENGINE forever loop")
        loops[0]["opcode"] = "control_repeat"
        loops[0]["inputs"]["TIMES"] = [1, [4, str(frames)]]
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as output:
            for entry in archive.infolist():
                output.writestr(entry, json.dumps(project).encode() if entry.filename == "project.json"
                                else archive.read(entry))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--frames", type=int, default=30)
    args = parser.parse_args()
    fixture(args.source, args.destination, args.frames)
