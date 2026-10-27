---
id: T-535
title: "Premier test matériel : trouver le ShowNET et ouvrir la sortie Art-Net (désarmée)"
status: todo
area: output
priority: P1
depends_on: [T-301]
owner: ""
branch: ""
source: session utilisateur 2026-10-27 (« trouver comment communiquer avec mon boîtier Laserworld et mon application »)
---

## Contexte
Michel veut que Laser Studio parle **directement** à son boîtier Laserworld
ShowNET, sans MadMapper. La seule voie directe et ouverte aujourd'hui est la
sortie **« ShowNET carte SD (Art-Net) »** (T-301, fusionnée) : le studio envoie
de l'ArtDmx sur le port Ethernet du boîtier, qui joue `NNN.ild` depuis sa
microSD. Le flux de points live du ShowNET est chiffré (NDA) : **hors
périmètre, ne jamais l'analyser, le capturer ni le rejouer** (cf.
`RAPPORT-BOITIER.md`, `CLAUDE.md`).

Cette tâche est **exceptionnelle** : elle touche le vrai boîtier, **à la
demande explicite de Michel, qui est présent devant le laser**. Elle reste
limitée à la découverte réseau et à l'ouverture de la sortie **désarmée**.

## À faire
Tourne sur le Mac de Michel (shell réel, avec le réseau). S'arrêter et
demander à Michel au moindre doute.

1. **Préparer** : MadMapper/Showcontroller fermés (ils occupent les ports de
   découverte). Boîtier allumé, câble Ethernet branché, clé/interlock du
   projecteur en position sûre.
2. **Trouver l'IP du boîtier** (lecture seule, n'émet rien) :
   - `python3 -I preuve-ecoute-passive.py` ~15 s, puis Ctrl-C ;
   - interfaces : `ifconfig | grep -E '^en|inet '` (quelle interface est sur
     le réseau du boîtier : `en0`, ou un adaptateur USB `en4`–`en6` ? voir
     `diagnostic_ethernet.sh`) ;
   - `arp -an` et `ping -c3 <IP>` (le rapport du 17/10 l'avait vu vers
     `192.168.129.51` en DHCP — à confirmer, elle peut avoir changé).
   - Si rien ne répond : le Mac est-il sur le même sous-réseau ? AutoIP
     (169.254.x.x) ? Le noter dans le Journal et s'arrêter.
3. **Lancer le studio sur le boîtier, désarmé** :
   `cargo run -p laser-studio -- --shownet-artnet <IP>`
   (port Art-Net par défaut 6454). Ne PAS passer d'option d'armement.
4. **Vérifier sans lumière** :
   - `GET /api/state` et `/api/frame` : `output_status` montre la sortie
     `artnet:<IP>:6454` ouverte, intensité 0, motif 0 ;
   - optionnel, lecture seule : `sudo tcpdump -i <iface> -n 'udp port 6454 and host <IP>'`
     doit montrer ~30 paquets/s vers le boîtier.
5. **S'arrêter là.** Écrire le résultat dans le Journal et dire à Michel que
   c'est prêt. **C'est Michel, et lui seul, qui arme** depuis l'interface,
   devant le laser, arrêt d'urgence à portée de main.

## Critères d'acceptation
- [ ] IP du boîtier et interface Mac identifiées et notées dans le Journal.
- [ ] Studio lancé avec `--shownet-artnet <IP>`, sortie ouverte, **désarmée**.
- [ ] Paquets ArtDmx observés vers le boîtier (ou raison notée sinon).
- [ ] Aucun armement, aucun faisceau allumé par l'agent.
- [ ] Rien d'ajouté au dépôt qui contienne IP réelle, capture ou trafic du
      boîtier (le Journal dit « IP trouvée sur le LAN », pas l'adresse, si le
      dépôt est public).

## Notes
- **Firmware** : l'unité est en `2016050502`. Si le boîtier ne réagit pas à
  l'Art-Net une fois armé par Michel, c'est probablement le firmware (pas de
  mode Art-Net-in / Professional). La mise à jour (Admin Tool ≥ 1.40,
  Windows, vers `2025120902`) est **une action de Michel**, jamais d'un agent.
- Le boîtier ne joue que ce qui est sur sa microSD : il faut la banque
  exportée par T-302 (`001.ild`…). Sans carte remplie, armé = rien ne
  s'affiche, c'est normal.
- Ne pas toucher aux DIP switches, à l'Admin Tool, ni au flux chiffré.

## Journal
- 2026-10-27 — tâche écrite (session Claude distante, sans accès réseau au
  boîtier) pour exécution par le Claude Code local de Michel.
