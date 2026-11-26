#!/usr/bin/env python3
import math
import struct
from pathlib import Path

folder = Path(__file__).resolve().parent
output = folder / "test_etoile.ild"

def make_star():
    corners = []
    for i in range(10):
        angle = math.pi / 2 + i * math.pi / 5
        radius = 25000 if i % 2 == 0 else 11000
        corners.append((
            round(radius * math.cos(angle)),
            round(radius * math.sin(angle))
        ))

    points = []
    for i, start in enumerate(corners):
        end = corners[(i + 1) % len(corners)]
        for j in range(12):
            t = j / 12
            x = round(start[0] + t * (end[0] - start[0]))
            y = round(start[1] + t * (end[1] - start[1]))
            points.append((x, y, 255, 255, 255))

    points.append((*corners[0], 255, 255, 255))
    return points

def make_header(fmt, name, count):
    return (
        b"ILDA" + bytes(3) + bytes([fmt])
        + name.encode("ascii")[:8].ljust(8, b" ")
        + b"PYTHON ".ljust(8, b" ")
        + struct.pack(">HHB", count, 0, 0)
        + bytes(3)
    )

def main():
    points = make_star()
    records = bytearray()

    for i, (x, y, r, g, b) in enumerate(points):
        status = 0x80 if i == len(points) - 1 else 0
        records.extend(struct.pack(">hhBBBB", x, y, status, b, g, r))

    data = (
        make_header(5, "ETOILE", len(points))
        + records
        + b"ILDA" + bytes(28)
    )
    output.write_bytes(data)

    # Relecture et contrôles structurels.
    check = output.read_bytes()
    assert check[:4] == b"ILDA"
    assert check[7] == 5
    assert struct.unpack(">H", check[24:26])[0] == len(points)
    assert len(check) == 64 + 8 * len(points)
    assert check[-32:] == b"ILDA" + bytes(28)
    assert check[32 + (len(points) - 1) * 8 + 4] & 0x80

    print("Fichier créé :", output)
    print("Format ILDA : 5 (True Color)")
    print("Nombre de points :", len(points))
    print("Taille :", len(check), "octets")
    print("TEST OK : structure vérifiée")
    print("Réseau : aucune transmission")
    print("Laser : aucune commande envoyée")

if __name__ == "__main__":
    main()
