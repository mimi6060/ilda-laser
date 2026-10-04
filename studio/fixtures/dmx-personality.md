# Laser Studio — personnalités DMX

<!-- Généré depuis studio/src/dmx/personality.rs par `cargo test -p laser-studio published_chart` (UPDATE_DMX_CHART=1 pour le réécrire). Ne pas modifier à la main. -->

Version de la table : 1.

Laser Studio se présente à une console lumière comme un projecteur : une instance par zone de projection, chacune à son adresse de départ (RÉGLAGES › Entrées réseau › *Appareils DMX*). Deux personnalités, de notre conception.

Règles communes :
- Aucun canal n'arme le laser, ne change un réglage de sécurité, une zone ou la calibration. Il n'y a pas de canal « allumer le laser », volontairement.
- Rien n'est appliqué tant que les *Entrées externes* sont fermées à l'écran, ni hors du périmètre DMX (*Maîtres* pour l'image, *Cues* pour page, cue et arrêt des cues).
- *Accès* sous 128 : l'instance n'obéit à rien, sauf *Fonction › noir*. L'image de la zone revient au neutre.
- 16 bits : le canal gros (poids fort) puis le canal fin.
- Les canaux centrés valent exactement le neutre à 128 (32768 en 16 bits).
- Lissage : taille, position et rotation sont interpolées entre deux trames reçues (60 ms par défaut, réglable par appareil). Page, cue, fonction et accès ne sont jamais lissés.
- Le stroboscope passe par le limiteur de stroboscope comme tout le reste.
- Intensité, taille, position, rotation et couleur s'appliquent à l'image de la zone, après les maîtres et avant la calibration et les étages de sécurité. La vitesse d'animation, les cues et l'arrêt des cues agissent sur tout le studio (les cues jouent sur leurs propres zones).

## Compact — 16 canaux

| Canal | Nom | Clé | Type | Défaut | Valeurs | Lissé |
|---:|---|---|---|---:|---|---|
| 1 | Intensité | `intensity` | niveau | 255 | 0 = noir, 255 = 100 % du maître | non |
| 2 | Taille | `size` | niveau | 128 | 0 = 0 %, 128 = 100 %, 255 = 200 % | oui |
| 3 | Position X | `pos_x` | niveau | 128 | 0 = bord gauche/bas, 128 = centre, 255 = bord droit/haut | oui |
| 4 | Position Y | `pos_y` | niveau | 128 | 0 = bord gauche/bas, 128 = centre, 255 = bord droit/haut | oui |
| 5 | Rotation | `rotation` | niveau | 128 | 0 = −180°, 128 = 0°, 255 = +180° | oui |
| 6 | Vitesse de rotation | `rot_speed` | niveau | 128 | 0 = −360 °/s, 128 = arrêt, 255 = +360 °/s | non |
| 7 | Vitesse d'animation | `anim_speed` | niveau | 64 | 64 = vitesse normale, 0 = figé, 255 ≈ ×4 (tout le studio) | non |
| 8 | Mode couleur | `color_mode` | numéro | 0 | plages de 32 : maître, fixe, teinte, palette, arc-en-ciel, chenillard | non |
| 9 | Teinte | `hue` | niveau | 0 | 0 à 255 = 0 à 360° | non |
| 10 | Stroboscope | `strobe` | niveau | 0 | 0–7 = ouvert, 8–255 = 1 à 20 Hz (limité par le limiteur de stroboscope) | non |
| 11 | Page | `page` | numéro | 0 | 0 = page affichée, N = page N | non |
| 12 | Cue | `cue` | numéro | 0 | 0 = aucun, 1–40 = case de la page (joue le cue à l'arrivée) | non |
| 13 | Taille X | `size_x` | niveau | 128 | 0 = 0 %, 128 = 100 %, 255 = 200 % | oui |
| 14 | Taille Y | `size_y` | niveau | 128 | 0 = 0 %, 128 = 100 %, 255 = 200 % | oui |
| 15 | Fonction | `function` | plages | 0 | 0–127 rien, 128–191 arrêter les cues, 192–255 noir (arrêt d'urgence) | non |
| 16 | Accès | `access` | accès | 0 | 0–127 n'obéit à rien (sauf noir), 128–255 obéit | non |

## Complet — 32 canaux

| Canal | Nom | Clé | Type | Défaut | Valeurs | Lissé |
|---:|---|---|---|---:|---|---|
| 1 | Intensité | `intensity` | niveau | 255 | 0 = noir, 255 = 100 % du maître | non |
| 2 | Taille | `size` | 16 bits (gros) | 128 | 0 = 0 %, 128 = 100 %, 255 = 200 % | oui |
| 3 | Taille (fin) | `size_fine` | 16 bits (fin) | 0 | octet de poids faible | oui |
| 4 | Position X | `pos_x` | 16 bits (gros) | 128 | 0 = bord gauche/bas, 128 = centre, 255 = bord droit/haut | oui |
| 5 | Position X (fin) | `pos_x_fine` | 16 bits (fin) | 0 | octet de poids faible | oui |
| 6 | Position Y | `pos_y` | 16 bits (gros) | 128 | 0 = bord gauche/bas, 128 = centre, 255 = bord droit/haut | oui |
| 7 | Position Y (fin) | `pos_y_fine` | 16 bits (fin) | 0 | octet de poids faible | oui |
| 8 | Rotation | `rotation` | 16 bits (gros) | 128 | 0 = −180°, 128 = 0°, 255 = +180° | oui |
| 9 | Rotation (fin) | `rotation_fine` | 16 bits (fin) | 0 | octet de poids faible | oui |
| 10 | Taille X | `size_x` | niveau | 128 | 0 = 0 %, 128 = 100 %, 255 = 200 % | oui |
| 11 | Taille Y | `size_y` | niveau | 128 | 0 = 0 %, 128 = 100 %, 255 = 200 % | oui |
| 12 | Vitesse de rotation | `rot_speed` | niveau | 128 | 0 = −360 °/s, 128 = arrêt, 255 = +360 °/s | non |
| 13 | Vitesse d'animation | `anim_speed` | niveau | 64 | 64 = vitesse normale, 0 = figé, 255 ≈ ×4 (tout le studio) | non |
| 14 | Mode couleur | `color_mode` | numéro | 0 | plages de 32 : maître, fixe, teinte, palette, arc-en-ciel, chenillard | non |
| 15 | Teinte | `hue` | niveau | 0 | 0 à 255 = 0 à 360° | non |
| 16 | Rouge | `red` | niveau | 255 | 0 à 255 | non |
| 17 | Vert | `green` | niveau | 255 | 0 à 255 | non |
| 18 | Bleu | `blue` | niveau | 255 | 0 à 255 | non |
| 19 | Dosage RVB | `rgb_mix` | niveau | 0 | 0 = couleurs du cue, 255 = couleur RVB seule | non |
| 20 | Stroboscope | `strobe` | niveau | 0 | 0–7 = ouvert, 8–255 = 1 à 20 Hz (limité par le limiteur de stroboscope) | non |
| 21 | Page | `page` | numéro | 0 | 0 = page affichée, N = page N | non |
| 22 | Cue | `cue` | numéro | 0 | 0 = aucun, 1–40 = case de la page (joue le cue à l'arrivée) | non |
| 23 | Effet 1 | `fx1` | numéro | 0 | 0 = aucun, 1 Vague, 2 Pulsation, 3 Balancement, 4 Torsion | non |
| 24 | Effet 1 : dosage | `fx1_amount` | niveau | 0 | 0 = sans effet, 255 = plein effet | non |
| 25 | Effet 2 | `fx2` | numéro | 0 | 0 = aucun, 1 Vague, 2 Pulsation, 3 Balancement, 4 Torsion | non |
| 26 | Effet 2 : dosage | `fx2_amount` | niveau | 0 | 0 = sans effet, 255 = plein effet | non |
| 27 | Effet 3 | `fx3` | numéro | 0 | 0 = aucun, 1 Vague, 2 Pulsation, 3 Balancement, 4 Torsion | non |
| 28 | Effet 3 : dosage | `fx3_amount` | niveau | 0 | 0 = sans effet, 255 = plein effet | non |
| 29 | Effet 4 | `fx4` | numéro | 0 | 0 = aucun, 1 Vague, 2 Pulsation, 3 Balancement, 4 Torsion | non |
| 30 | Effet 4 : dosage | `fx4_amount` | niveau | 0 | 0 = sans effet, 255 = plein effet | non |
| 31 | Fonction | `function` | plages | 0 | 0–127 rien, 128–191 arrêter les cues, 192–255 noir (arrêt d'urgence) | non |
| 32 | Accès | `access` | accès | 0 | 0–127 n'obéit à rien (sauf noir), 128–255 obéit | non |

