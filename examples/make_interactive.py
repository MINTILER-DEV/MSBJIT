"""Rebuild interactive.sb3 using only Python's standard library."""
import hashlib
import json
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile


def block(opcode, next=None, inputs=None, fields=None, hat=False):
    return dict(opcode=opcode, next=next, parent=None, inputs=inputs or {},
                fields=fields or {}, shadow=False, topLevel=hat)


def number(value):
    return [1, [4, str(value)]]


assets = {}


def costume(name, svg, center):
    data = svg.encode()
    asset_id = hashlib.md5(data).hexdigest()
    filename = asset_id + ".svg"
    assets[filename] = data
    return dict(name=name, assetId=asset_id, md5ext=filename, dataFormat="svg",
                bitmapResolution=1, rotationCenterX=center[0], rotationCenterY=center[1])


backdrop = costume("Grid", '''<svg xmlns="http://www.w3.org/2000/svg" width="480" height="360">
<defs><pattern id="g" width="40" height="40" patternUnits="userSpaceOnUse"><path d="M40 0H0V40" fill="none" stroke="#d6deec"/></pattern></defs>
<path fill="#eef2ff" d="M0 0h480v360H0z"/><path fill="url(#g)" d="M0 0h480v360H0z"/>
<path d="M240 0v360M0 180h480" stroke="#a5b4fc"/></svg>''', (240, 180))
sprite = costume("Blue", '''<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
<circle cx="32" cy="32" r="30" fill="#6366f1"/><path d="M20 26h6m12 0h6M20 40q12 12 24 0" stroke="white" stroke-width="5" fill="none" stroke-linecap="round"/></svg>''', (32, 32))
sprite2 = costume("Pink", '''<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
<circle cx="32" cy="32" r="30" fill="#ec4899"/><path d="M20 26h6m12 0h6M20 40q12 12 24 0" stroke="white" stroke-width="5" fill="none" stroke-linecap="round"/></svg>''', (32, 32))
blocks = {
    "flag": block("event_whenflagclicked", "home", hat=True),
    "home": block("motion_gotoxy", inputs={"X": number(0), "Y": number(0)}),
    "click": block("event_whenthisspriteclicked", "costume", hat=True),
    "costume": block("looks_nextcostume"),
    "space": block("event_whenkeypressed", "follow", fields={"KEY_OPTION": ["space", None]}, hat=True),
    "follow": block("motion_gotoxy", inputs={"X": [2, "mx"], "Y": [2, "my"]}),
    "mx": block("sensing_mousex"), "my": block("sensing_mousey"),
}
for name, opcode, field, amount in [
    ("left arrow", "motion_changexby", "DX", -15),
    ("right arrow", "motion_changexby", "DX", 15),
    ("up arrow", "motion_changeyby", "DY", 15),
    ("down arrow", "motion_changeyby", "DY", -15),
]:
    blocks[name] = block("event_whenkeypressed", name + " move", fields={"KEY_OPTION": [name, None]}, hat=True)
    blocks[name + " move"] = block(opcode, inputs={field: number(amount)})

project = {"targets": [
    dict(name="Stage", isStage=True, variables={}, lists={}, broadcasts={}, blocks={},
         costumes=[backdrop], currentCostume=0, sounds=[], volume=100, layerOrder=0),
    dict(name="Player", isStage=False, variables={}, lists={}, broadcasts={}, blocks=blocks,
         costumes=[sprite, sprite2], currentCostume=0, sounds=[], volume=100, layerOrder=1,
         x=0, y=0, size=100, direction=90, visible=True, draggable=False, rotationStyle="all around"),
], "monitors": [], "extensions": [], "meta": {"semver": "3.0.0", "vm": "0.2.0", "agent": "MSBJIT"}}

with ZipFile(Path(__file__).with_name("interactive.sb3"), "w", ZIP_DEFLATED) as archive:
    archive.writestr("project.json", json.dumps(project))
    for filename, data in assets.items():
        archive.writestr(filename, data)
