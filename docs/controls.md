# Identifiants de contrôle

_Généré par `cargo test -p laser-studio controls` — ne pas éditer à la main._

| id | libellé | groupe | type | externe |
|---|---|---|---|---|
| `look.size` | Taille du look | look | continu 0.05…1 | oui |
| `look.brightness` | Luminosité du look | look | continu 0…1 | oui |
| `look.rotation_speed` | Rotation du look | look | continu -360…360 | oui |
| `master.brightness` | Luminosité maître | master | continu 0…1 | oui |
| `master.size` | Taille maître | master | continu 0…2 | oui |
| `master.size_x` | Taille X | master | continu -2…2 | oui |
| `master.size_y` | Taille Y | master | continu -2…2 | oui |
| `master.pos_x` | Position X | master | continu -1…1 | oui |
| `master.pos_y` | Position Y | master | continu -1…1 | oui |
| `master.rot_x.angle` | Angle X | master | continu -180…180 | oui |
| `master.rot_x.speed` | Rotation X | master | continu -720…720 | oui |
| `master.rot_y.angle` | Angle Y | master | continu -180…180 | oui |
| `master.rot_y.speed` | Rotation Y | master | continu -720…720 | oui |
| `master.rot_z.angle` | Angle Z | master | continu -180…180 | oui |
| `master.rot_z.speed` | Rotation Z | master | continu -720…720 | oui |
| `master.rot.preset` | Vitesse de rotation | master | choix : Stop / Lent / Moyen / Rapide | oui |
| `master.rot.sync` | Rotation synchro tempo | master | bascule | oui |
| `master.rot.reverse` | Inverser la rotation | master | momentané | oui |
| `master.perspective` | Perspective | master | continu 0…1 | oui |
| `master.speed` | Vitesse d'animation | master | continu 0…4 | oui |
| `master.reset` | Réinitialiser le direct | master | déclencheur | oui |
| `master.color.mode` | Mode couleur | master | choix : Normal / Fixe / Teinte / Palette / Arc-en-ciel / Chenillard | oui |
| `master.color.hue` | Teinte | master | continu 0…360 | oui |
| `master.color.palette` | Palette | master | choix : Froid / Chaud / Feu / Océan / Néon / Forêt / Tricolore / Blanc pur / Perso 1 / Perso 2 / Perso 3 / Perso 4 / Perso 5 / Perso 6 / Perso 7 / Perso 8 | oui |
| `master.color.palette_mode` | Mode de palette | master | choix : Plus proche / Pas à pas | oui |
| `master.color.offset` | Décalage de palette | master | continu 0…15 | oui |
| `master.color.rate` | Pas couleur (temps) | master | choix : 1/8 / 1/4 / 1/2 / 1 / 2 / 4 | oui |
| `master.color.rate_hz` | Vitesse couleur libre | master | continu 0…10 | oui |
| `master.color.spread` | Étalement arc-en-ciel | master | continu 0…4 | oui |
| `master.color.chase_spread` | Répartition du chenillard | master | choix : Tout / Par trait / Par point | oui |
| `master.color.red` | Couleur fixe : Rouge | master | continu 0…1 | oui |
| `master.color.green` | Couleur fixe : Vert | master | continu 0…1 | oui |
| `master.color.blue` | Couleur fixe : Bleu | master | continu 0…1 | oui |
| `audio.enabled` | Réagit à la musique | audio | bascule | oui |
| `audio.size` | Taille suit les basses | audio | continu 0…1 | oui |
| `audio.rotate` | Rotation suit les basses | audio | continu 0…1 | oui |
| `audio.flash` | Flash sur le beat | audio | continu 0…1 | oui |
| `audio.color_on_beat` | Couleur change au beat | audio | bascule | oui |
| `transport.blackout` | Blackout | transport | déclencheur | oui |
| `transport.arm` | Allumer le laser | transport | bascule | non |
| `tempo.tap` | Tap tempo | tempo | déclencheur | oui |
| `tempo.resync` | Recaler sur le 1 | tempo | déclencheur | oui |
| `tempo.bpm` | BPM | tempo | continu 40…250 | oui |
| `tempo.nudge_up` | Avancer la phase | tempo | déclencheur | oui |
| `tempo.nudge_down` | Retarder la phase | tempo | déclencheur | oui |
| `tempo.double` | Tempo ×2 | tempo | déclencheur | oui |
| `tempo.half` | Tempo ÷2 | tempo | déclencheur | oui |
| `page.next` | Page de cues suivante | page | déclencheur | oui |
| `page.prev` | Page de cues précédente | page | déclencheur | oui |
| `page.1` | Page Abstraits | page | déclencheur | oui |
| `page.2` | Page Tunnels | page | déclencheur | oui |
| `page.3` | Page Faisceaux | page | déclencheur | oui |
| `page.4` | Page Balayages | page | déclencheur | oui |
| `page.5` | Page Vagues | page | déclencheur | oui |
| `page.6` | Page Géométrie | page | déclencheur | oui |
| `page.7` | Page Audio | page | déclencheur | oui |
| `page.8` | Page Texte & horloge | page | déclencheur | oui |
| `cue.mode` | Mode de clic des cues | cue | choix : Basculer / Flash / Solo / Relancer | oui |
| `cue.multi` | Plusieurs cues à la fois | cue | bascule | oui |
| `cue.max_active` | Cues simultanés max | cue | continu 1…16 | oui |
| `cue.stop_all` | Arrêter tous les cues | cue | déclencheur | oui |
| `grid.1.1.1` | Lissajous 1:2 · vert | grid | momentané | oui |
| `grid.1.1.2` | Lissajous 1:2 · arc-en-ciel | grid | momentané | oui |
| `grid.1.1.3` | Lissajous 1:2 · cyan → magenta | grid | momentané | oui |
| `grid.1.1.4` | Lissajous 2:3 · vert | grid | momentané | oui |
| `grid.1.1.5` | Lissajous 2:3 · arc-en-ciel | grid | momentané | oui |
| `grid.1.1.6` | Lissajous 2:3 · cyan → magenta | grid | momentané | oui |
| `grid.1.1.7` | Lissajous 3:4 · vert | grid | momentané | oui |
| `grid.1.1.8` | Lissajous 3:4 · arc-en-ciel | grid | momentané | oui |
| `grid.1.2.1` | Lissajous 3:4 · cyan → magenta | grid | momentané | oui |
| `grid.1.2.2` | Lissajous 3:5 · vert | grid | momentané | oui |
| `grid.1.2.3` | Lissajous 3:5 · arc-en-ciel | grid | momentané | oui |
| `grid.1.2.4` | Lissajous 3:5 · cyan → magenta | grid | momentané | oui |
| `grid.1.2.5` | Lissajous 4:5 · vert | grid | momentané | oui |
| `grid.1.2.6` | Lissajous 4:5 · arc-en-ciel | grid | momentané | oui |
| `grid.1.2.7` | Lissajous 4:5 · cyan → magenta | grid | momentané | oui |
| `grid.1.2.8` | Lissajous 5:6 · vert | grid | momentané | oui |
| `grid.1.3.1` | Lissajous 5:6 · arc-en-ciel | grid | momentané | oui |
| `grid.1.3.2` | Lissajous 5:6 · cyan → magenta | grid | momentané | oui |
| `grid.1.3.3` | Lissajous 1:3 · vert | grid | momentané | oui |
| `grid.1.3.4` | Lissajous 1:3 · arc-en-ciel | grid | momentané | oui |
| `grid.1.3.5` | Lissajous 1:3 · cyan → magenta | grid | momentané | oui |
| `grid.1.3.6` | Lissajous 2:5 · vert | grid | momentané | oui |
| `grid.1.3.7` | Lissajous 2:5 · arc-en-ciel | grid | momentané | oui |
| `grid.1.3.8` | Lissajous 2:5 · cyan → magenta | grid | momentané | oui |
| `grid.1.4.1` | Spirographe 2.5 / 0.6 · arc-en-ciel | grid | momentané | oui |
| `grid.1.4.2` | Spirographe 2.5 / 1.2 · arc-en-ciel | grid | momentané | oui |
| `grid.1.4.3` | Spirographe 3 / 0.6 · cyan | grid | momentané | oui |
| `grid.1.4.4` | Spirographe 3 / 1.2 · cyan | grid | momentané | oui |
| `grid.1.4.5` | Spirographe 3.5 / 0.6 · arc-en-ciel | grid | momentané | oui |
| `grid.1.4.6` | Spirographe 3.5 / 1.2 · arc-en-ciel | grid | momentané | oui |
| `grid.1.4.7` | Spirographe 4 / 0.6 · cyan | grid | momentané | oui |
| `grid.1.4.8` | Spirographe 4 / 1.2 · cyan | grid | momentané | oui |
| `grid.1.5.1` | Spirographe 5 / 0.6 · arc-en-ciel | grid | momentané | oui |
| `grid.1.5.2` | Spirographe 5 / 1.2 · arc-en-ciel | grid | momentané | oui |
| `grid.1.5.3` | Spirographe 7 / 0.6 · cyan | grid | momentané | oui |
| `grid.1.5.4` | Spirographe 7 / 1.2 · cyan | grid | momentané | oui |
| `grid.1.5.5` | Rosace 3 pétales · rouge | grid | momentané | oui |
| `grid.1.5.6` | Rosace 3 pétales · arc-en-ciel | grid | momentané | oui |
| `grid.1.5.7` | Rosace 4 pétales · rouge | grid | momentané | oui |
| `grid.1.5.8` | Rosace 4 pétales · arc-en-ciel | grid | momentané | oui |
| `grid.2.1.1` | Tunnel 4 anneaux · cyan | grid | momentané | oui |
| `grid.2.1.2` | Tunnel 4 anneaux · rouge / bleu | grid | momentané | oui |
| `grid.2.1.3` | Tunnel 4 anneaux · arc-en-ciel | grid | momentané | oui |
| `grid.2.1.4` | Tunnel 6 anneaux · cyan | grid | momentané | oui |
| `grid.2.1.5` | Tunnel 6 anneaux · rouge / bleu | grid | momentané | oui |
| `grid.2.1.6` | Tunnel 6 anneaux · arc-en-ciel | grid | momentané | oui |
| `grid.2.1.7` | Tunnel 8 anneaux · cyan | grid | momentané | oui |
| `grid.2.1.8` | Tunnel 8 anneaux · rouge / bleu | grid | momentané | oui |
| `grid.2.2.1` | Tunnel 8 anneaux · arc-en-ciel | grid | momentané | oui |
| `grid.2.2.2` | Tunnel 3 côtés · cyan → magenta | grid | momentané | oui |
| `grid.2.2.3` | Tunnel 3 côtés torsadé · arc-en-ciel | grid | momentané | oui |
| `grid.2.2.4` | Tunnel 4 côtés · cyan → magenta | grid | momentané | oui |
| `grid.2.2.5` | Tunnel 4 côtés torsadé · arc-en-ciel | grid | momentané | oui |
| `grid.2.2.6` | Tunnel 5 côtés · cyan → magenta | grid | momentané | oui |
| `grid.2.2.7` | Tunnel 5 côtés torsadé · arc-en-ciel | grid | momentané | oui |
| `grid.2.2.8` | Tunnel 6 côtés · cyan → magenta | grid | momentané | oui |
| `grid.2.3.1` | Tunnel 6 côtés torsadé · arc-en-ciel | grid | momentané | oui |
| `grid.2.3.2` | Tunnel 8 côtés · cyan → magenta | grid | momentané | oui |
| `grid.2.3.3` | Tunnel 8 côtés torsadé · arc-en-ciel | grid | momentané | oui |
| `grid.2.3.4` | Anneaux pulsés 3 · vert | grid | momentané | oui |
| `grid.2.3.5` | Anneaux pulsés 3 · vert / jaune | grid | momentané | oui |
| `grid.2.3.6` | Anneaux pulsés 5 · vert | grid | momentané | oui |
| `grid.2.3.7` | Anneaux pulsés 5 · vert / jaune | grid | momentané | oui |
| `grid.3.1.1` | Éventail 3 · vert | grid | momentané | oui |
| `grid.3.1.2` | Éventail 3 · bleu | grid | momentané | oui |
| `grid.3.1.3` | Éventail 3 · rouge | grid | momentané | oui |
| `grid.3.1.4` | Éventail 3 · arc-en-ciel | grid | momentané | oui |
| `grid.3.1.5` | Éventail 5 · vert | grid | momentané | oui |
| `grid.3.1.6` | Éventail 5 · bleu | grid | momentané | oui |
| `grid.3.1.7` | Éventail 5 · rouge | grid | momentané | oui |
| `grid.3.1.8` | Éventail 5 · arc-en-ciel | grid | momentané | oui |
| `grid.3.2.1` | Éventail 8 · vert | grid | momentané | oui |
| `grid.3.2.2` | Éventail 8 · bleu | grid | momentané | oui |
| `grid.3.2.3` | Éventail 8 · rouge | grid | momentané | oui |
| `grid.3.2.4` | Éventail 8 · arc-en-ciel | grid | momentané | oui |
| `grid.3.2.5` | Éventail 12 · vert | grid | momentané | oui |
| `grid.3.2.6` | Éventail 12 · bleu | grid | momentané | oui |
| `grid.3.2.7` | Éventail 12 · rouge | grid | momentané | oui |
| `grid.3.2.8` | Éventail 12 · arc-en-ciel | grid | momentané | oui |
| `grid.3.3.1` | Cône 6 · cyan | grid | momentané | oui |
| `grid.3.3.2` | Cône 6 · rouge / bleu | grid | momentané | oui |
| `grid.3.3.3` | Cône 6 · arc-en-ciel | grid | momentané | oui |
| `grid.3.3.4` | Cône 10 · cyan | grid | momentané | oui |
| `grid.3.3.5` | Cône 10 · rouge / bleu | grid | momentané | oui |
| `grid.3.3.6` | Cône 10 · arc-en-ciel | grid | momentané | oui |
| `grid.3.3.7` | Cône 16 · cyan | grid | momentané | oui |
| `grid.3.3.8` | Cône 16 · rouge / bleu | grid | momentané | oui |
| `grid.3.4.1` | Cône 16 · arc-en-ciel | grid | momentané | oui |
| `grid.3.4.2` | Vague de faisceaux 8 · vert | grid | momentané | oui |
| `grid.3.4.3` | Vague de faisceaux 8 · vert / jaune | grid | momentané | oui |
| `grid.3.4.4` | Vague de faisceaux 8 · arc-en-ciel | grid | momentané | oui |
| `grid.3.4.5` | Vague de faisceaux 12 · vert | grid | momentané | oui |
| `grid.3.4.6` | Vague de faisceaux 12 · vert / jaune | grid | momentané | oui |
| `grid.3.4.7` | Vague de faisceaux 12 · arc-en-ciel | grid | momentané | oui |
| `grid.4.1.1` | Balayage · vert | grid | momentané | oui |
| `grid.4.1.2` | Balayage · rouge | grid | momentané | oui |
| `grid.4.1.3` | Balayage · cyan | grid | momentané | oui |
| `grid.4.1.4` | Balayage · arc-en-ciel | grid | momentané | oui |
| `grid.4.1.5` | Nappe (liquid sky) · vert | grid | momentané | oui |
| `grid.4.1.6` | Nappe (liquid sky) · bleu | grid | momentané | oui |
| `grid.4.1.7` | Nappe (liquid sky) · cyan → magenta | grid | momentané | oui |
| `grid.4.1.8` | Nappe (liquid sky) · arc-en-ciel | grid | momentané | oui |
| `grid.4.2.1` | Lignes de balayage 3 · vert | grid | momentané | oui |
| `grid.4.2.2` | Lignes de balayage 3 · rouge / bleu | grid | momentané | oui |
| `grid.4.2.3` | Grille de balayage 3 · vert | grid | momentané | oui |
| `grid.4.2.4` | Grille de balayage 3 · rouge / bleu | grid | momentané | oui |
| `grid.4.2.5` | Lignes de balayage 5 · vert | grid | momentané | oui |
| `grid.4.2.6` | Lignes de balayage 5 · rouge / bleu | grid | momentané | oui |
| `grid.4.2.7` | Grille de balayage 5 · vert | grid | momentané | oui |
| `grid.4.2.8` | Grille de balayage 5 · rouge / bleu | grid | momentané | oui |
| `grid.5.1.1` | Oscillateurs ×2 · cyan | grid | momentané | oui |
| `grid.5.1.2` | Oscillateurs ×2 · arc-en-ciel | grid | momentané | oui |
| `grid.5.1.3` | Oscillateurs ×2 · vert → rouge | grid | momentané | oui |
| `grid.5.1.4` | Oscillateurs ×3 · cyan | grid | momentané | oui |
| `grid.5.1.5` | Oscillateurs ×3 · arc-en-ciel | grid | momentané | oui |
| `grid.5.1.6` | Oscillateurs ×3 · vert → rouge | grid | momentané | oui |
| `grid.5.1.7` | Oscillateurs ×5 · cyan | grid | momentané | oui |
| `grid.5.1.8` | Oscillateurs ×5 · arc-en-ciel | grid | momentané | oui |
| `grid.5.2.1` | Oscillateurs ×5 · vert → rouge | grid | momentané | oui |
| `grid.5.2.2` | Hélice ADN · vert | grid | momentané | oui |
| `grid.5.2.3` | Hélice ADN · rouge / bleu | grid | momentané | oui |
| `grid.5.2.4` | Hélice ADN · cyan → magenta | grid | momentané | oui |
| `grid.5.2.5` | Hélice ADN · arc-en-ciel | grid | momentané | oui |
| `grid.5.2.6` | Spirale 2 bras · vert | grid | momentané | oui |
| `grid.5.2.7` | Spirale 2 bras · arc-en-ciel | grid | momentané | oui |
| `grid.5.2.8` | Spirale 3 bras · vert | grid | momentané | oui |
| `grid.5.3.1` | Spirale 3 bras · arc-en-ciel | grid | momentané | oui |
| `grid.5.3.2` | Spirale 4 bras · vert | grid | momentané | oui |
| `grid.5.3.3` | Spirale 4 bras · arc-en-ciel | grid | momentané | oui |
| `grid.5.3.4` | Spirale 6 bras · vert | grid | momentané | oui |
| `grid.5.3.5` | Spirale 6 bras · arc-en-ciel | grid | momentané | oui |
| `grid.5.3.6` | Soleil 8 rayons · jaune | grid | momentané | oui |
| `grid.5.3.7` | Soleil 8 rayons · arc-en-ciel | grid | momentané | oui |
| `grid.5.3.8` | Soleil 12 rayons · jaune | grid | momentané | oui |
| `grid.5.4.1` | Soleil 12 rayons · arc-en-ciel | grid | momentané | oui |
| `grid.5.4.2` | Soleil 24 rayons · jaune | grid | momentané | oui |
| `grid.5.4.3` | Soleil 24 rayons · arc-en-ciel | grid | momentané | oui |
| `grid.6.1.1` | Cercle · vert | grid | momentané | oui |
| `grid.6.1.2` | Cercle qui tourne · rouge | grid | momentané | oui |
| `grid.6.1.3` | Cercle qui tourne · bleu | grid | momentané | oui |
| `grid.6.1.4` | Carré · vert | grid | momentané | oui |
| `grid.6.1.5` | Carré qui tourne · rouge | grid | momentané | oui |
| `grid.6.1.6` | Carré qui tourne · bleu | grid | momentané | oui |
| `grid.6.1.7` | Triangle · vert | grid | momentané | oui |
| `grid.6.1.8` | Triangle qui tourne · rouge | grid | momentané | oui |
| `grid.6.2.1` | Triangle qui tourne · bleu | grid | momentané | oui |
| `grid.6.2.2` | Étoile · vert | grid | momentané | oui |
| `grid.6.2.3` | Étoile qui tourne · rouge | grid | momentané | oui |
| `grid.6.2.4` | Étoile qui tourne · bleu | grid | momentané | oui |
| `grid.7.1.1` | Spectre 6 barres · vert → rouge | grid | momentané | oui |
| `grid.7.1.2` | Spectre 6 barres · arc-en-ciel | grid | momentané | oui |
| `grid.7.1.3` | Spectre 10 barres · vert → rouge | grid | momentané | oui |
| `grid.7.1.4` | Spectre 10 barres · arc-en-ciel | grid | momentané | oui |
| `grid.7.1.5` | Spectre 14 barres · vert → rouge | grid | momentané | oui |
| `grid.7.1.6` | Spectre 14 barres · arc-en-ciel | grid | momentané | oui |
| `grid.7.1.7` | Rosace qui pulse · arc-en-ciel | grid | momentané | oui |
| `grid.7.1.8` | Soleil qui pulse · jaune | grid | momentané | oui |
| `grid.7.2.1` | Tunnel au rythme · rouge / bleu | grid | momentané | oui |
| `grid.7.2.2` | Éventail au rythme · vert | grid | momentané | oui |
| `grid.7.2.3` | Oscillateurs au rythme · cyan → magenta | grid | momentané | oui |
| `grid.7.2.4` | Spirale au rythme · arc-en-ciel | grid | momentané | oui |
| `grid.7.2.5` | Vortex au rythme · cyan → magenta | grid | momentané | oui |
| `grid.7.2.6` | Cône au rythme · rouge / bleu | grid | momentané | oui |
| `grid.7.2.7` | Anneaux au rythme · vert | grid | momentané | oui |
| `grid.8.1.1` | Horloge · blanc | grid | momentané | oui |
| `grid.8.1.2` | Horloge · vert | grid | momentané | oui |
| `grid.8.1.3` | Horloge · arc-en-ciel | grid | momentané | oui |
| `grid.8.1.4` | HELLO · vert | grid | momentané | oui |
| `grid.8.1.5` | HELLO · rouge | grid | momentané | oui |
| `grid.8.1.6` | HELLO · cyan | grid | momentané | oui |
| `grid.8.1.7` | PARTY · vert | grid | momentané | oui |
| `grid.8.1.8` | PARTY · rouge | grid | momentané | oui |
| `grid.8.2.1` | PARTY · cyan | grid | momentané | oui |
| `grid.8.2.2` | DJ · vert | grid | momentané | oui |
| `grid.8.2.3` | DJ · rouge | grid | momentané | oui |
| `grid.8.2.4` | DJ · cyan | grid | momentané | oui |
| `grid.8.2.5` | LOVE · vert | grid | momentané | oui |
| `grid.8.2.6` | LOVE · rouge | grid | momentané | oui |
| `grid.8.2.7` | LOVE · cyan | grid | momentané | oui |
| `grid.8.2.8` | WOW · vert | grid | momentané | oui |
| `grid.8.3.1` | WOW · rouge | grid | momentané | oui |
| `grid.8.3.2` | WOW · cyan | grid | momentané | oui |
| `grid.8.3.3` | MERCI · vert | grid | momentané | oui |
| `grid.8.3.4` | MERCI · rouge | grid | momentané | oui |
| `grid.8.3.5` | MERCI · cyan | grid | momentané | oui |
