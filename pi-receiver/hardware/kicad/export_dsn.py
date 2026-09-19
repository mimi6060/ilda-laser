#!/usr/bin/env python3
"""Exports board.kicad_pcb (the clean placement) as a Specctra DSN file for
Freerouting. Widens the default trace width/clearance from KiCad's 0.2mm
default to 0.25mm afterwards - this board has plenty of room and no reason
to push fab tolerances (see DESIGN.md)."""
import pcbnew

board = pcbnew.LoadBoard("board.kicad_pcb")
ok = pcbnew.ExportSpecctraDSN(board, "board.dsn")
print("export ok:", ok)

content = open("board.dsn").read()
content = content.replace(
    "(width 200)\n      (clearance 200)\n      (clearance 50 (type smd_smd))",
    "(width 250)\n      (clearance 250)\n      (clearance 100 (type smd_smd))",
)
content = content.replace(
    "(rule\n        (width 200)\n        (clearance 200)\n      )",
    "(rule\n        (width 250)\n        (clearance 250)\n      )",
)
open("board.dsn", "w").write(content)
print("widened default trace width/clearance to 0.25mm")
