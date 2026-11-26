from pathlib import Path
import struct

p = Path(__file__).with_name("test_etoile.ild")
data = p.read_bytes()

assert data[:4] == b"ILDA", "Signature ILDA invalide"
assert data[7] == 5, "Le fichier n'est pas au format 5"

n = struct.unpack(">H", data[24:26])[0]
assert len(data) == 32 + n * 8 + 32, "Taille incohérente"
assert data[-32:-28] == b"ILDA", "Marqueur de fin absent"

print("Fichier :", p)
print("Points :", n)
print("Format : ILDA True Color")
print("Vérification : OK")
print("Aucune donnée envoyée au matériel.")
