"""Make a finite sub1k render benchmark without changing rendering quality.

The MAIN ENGINE's outer forever becomes repeat N. Its render broadcast waits
for completion, and its frame-counter increment moves after the render call,
so host warp preemption cannot count an aborted/restarted frame. No input is
supplied; the camera stays at its initialized position (optionally overridden
with --rot-x). Rendering procedures, quality settings and assets are unchanged.
No project assets are copied into the repository. Run the ignored
sub1k_complete_frames benchmark with MSBJIT_PEN_BENCH_PROJECT pointing at the
resulting archive.
"""
import argparse
import json
from pathlib import Path
import zipfile


def fixture(source, destination, frames, rot_x=None):
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
        engine_blocks = engine["blocks"]
        increments = [(key, block) for key, block in engine_blocks.items()
                      if block["opcode"] == "data_changevariableby"
                      and block["fields"]["VARIABLE"][0] == "System: Frames Elapsed"]
        if len(increments) != 1:
            raise ValueError("expected one frame counter increment")
        key, increment = increments[0]
        engine_blocks[increment["parent"]]["next"] = increment["next"]
        engine_blocks[increment["next"]]["parent"] = increment["parent"]
        del engine_blocks[key]
        renderer = next(t for t in project["targets"] if t["name"] == "E: Renderer")
        hats = [b for b in renderer["blocks"].values()
                if b["opcode"] == "event_whenbroadcastreceived"
                and b["fields"]["BROADCAST_OPTION"][0] == "TICK: Render 3D"]
        if len(hats) != 1:
            raise ValueError("expected one render receiver")
        call_key = hats[0]["next"]
        call = renderer["blocks"][call_key]
        if call["opcode"] != "procedures_call" or call["next"] is not None:
            raise ValueError("expected one render procedure call")
        call["next"] = key
        increment["parent"], increment["next"] = call_key, None
        renderer["blocks"][key] = increment
        broadcasts = [b for b in engine_blocks.values()
                      if b["opcode"] == "event_broadcast"
                      and b["inputs"]["BROADCAST_INPUT"][1][1] == "TICK: Render 3D"]
        if len(broadcasts) != 1:
            raise ValueError("expected one render broadcast")
        broadcasts[0]["opcode"] = "event_broadcastandwait"
        if rot_x is not None:
            resets = [b for b in engine["blocks"].values()
                      if b["opcode"] == "data_setvariableto"
                      and b["fields"]["VARIABLE"][0] == "Camera: RotX"]
            if len(resets) != 1:
                raise ValueError("expected one camera RotX initializer")
            resets[0]["inputs"]["VALUE"] = [1, [4, str(rot_x)]]
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as output:
            for entry in archive.infolist():
                output.writestr(entry, json.dumps(project).encode() if entry.filename == "project.json"
                                else archive.read(entry))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--frames", type=int, default=30)
    parser.add_argument("--rot-x", type=float, help="optional fixed camera pitch; quality is unchanged")
    args = parser.parse_args()
    fixture(args.source, args.destination, args.frames, args.rot_x)
