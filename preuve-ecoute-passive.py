#!/usr/bin/env python3
# Preuve PASSIVE (lecture seule) : montre que n'importe quel process du LAN
# reçoit, EN CLAIR et SANS AUTHENTIFICATION, les annonces de découverte du
# contrôleur laser (broadcast "Search Moncha.NET", IDN-Hello, etc.).
#
# N'ÉMET RIEN. N'actionne aucun laser. Ne touche pas au flux de contrôle
# chiffré du ShowNET. Simple socket UDP en écoute sur les ports de découverte.
#
# Usage :  python3 -I preuve-ecoute-passive.py
# Arrêt :  Ctrl-C
#
# Lance-le pendant que MadMapper/Showcontroller tourne. Chaque ligne affichée
# est un paquet qu'un attaquant sur le même réseau recevrait aussi, sans rien
# faire d'autre que d'être branché au réseau.

import socket
import datetime

PORTS = [25000, 7255, 41855, 45457]  # ports de découverte observés

def listener(port):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    except (AttributeError, OSError):
        pass
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.bind(("", port))
    s.setblocking(False)
    return s

def main():
    import select
    socks = {}
    for p in PORTS:
        try:
            socks[listener(p)] = p
        except OSError as e:
            print(f"[!] port {p} indisponible ({e}) — peut-être déjà pris, essaie avec l'appli fermée")
    if not socks:
        print("Aucun port en écoute. Arrêt.")
        return
    print(f"Écoute passive sur UDP {sorted(socks.values())} — Ctrl-C pour arrêter.")
    print("Chaque ligne = un paquet reçu SANS authentification.\n")
    try:
        while True:
            ready, _, _ = select.select(list(socks), [], [], 1.0)
            for s in ready:
                data, addr = s.recvfrom(2048)
                port = socks[s]
                ts = datetime.datetime.now().strftime("%H:%M:%S")
                # on affiche seulement les octets imprimables, le reste en points
                ascii_view = "".join(chr(b) if 32 <= b < 127 else "." for b in data)
                print(f"{ts}  de {addr[0]:<15} vers :{port:<6} ({len(data):>2} o)  {ascii_view!r}")
    except KeyboardInterrupt:
        print("\nArrêt. Preuve : ces annonces sont reçues par tout process, sans identifiant.")

if __name__ == "__main__":
    main()
