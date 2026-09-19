#!/usr/bin/env python3
"""Imports Freerouting's board_routed.ses (routed from board.dsn) back
onto board.kicad_pcb and saves the result as board_freerouted.kicad_pcb."""
import pcbnew

board = pcbnew.LoadBoard("board.kicad_pcb")
ok = pcbnew.ImportSpecctraSES(board, "board_routed.ses")
print("import ok:", ok)
pcbnew.SaveBoard("board_freerouted.kicad_pcb", board)
print("saved board_freerouted.kicad_pcb")
