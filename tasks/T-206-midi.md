---
id: T-206
title: Couleurs RGB des pads sur l'APC40 mkII (couleur de la cue, pulsation)
status: todo
area: midi
priority: P2
depends_on: [T-205]
owner: ""
branch: ""
source: docs/research/midi-apc40.md#23-outbound-rgb-pads-and-other-leds
---

## Contexte
Les pads de l'APC40 mkII sont RGB : chaque pad peut prendre la couleur de sa
cue, ce qui permet de choisir un look à l'œil sans regarder l'écran. La cue
en cours pulse, les pages ont chacune leur couleur.

## À faire
- Table de la palette mkII (128 vélocités → sRGB, tableau du rapport §2.4,
  tiré du protocole Akai v1.2) dans `midi_led.rs`.
- `fn nearest_velocity(rgb: [u8; 3]) -> u8` : couleur de palette la plus
  proche, en ignorant les entrées sombres (≤ `#2B`), distance pondérée
  (2·ΔR² + 4·ΔG² + 3·ΔB² suffit) ; résultat mis en cache par couleur.
- Variante « atténuée » d'une vélocité : pour les familles 4–59 (pas de 4),
  la couleur pleine est `4k+1` et la version sombre `4k+2` ; pour les
  autres, table de correspondance à la main (ou même vélocité si rien
  d'adapté).
- Couleur représentative d'une cue : `settings.color` ; pour les modes
  arc-en-ciel/dégradé/alternance, la couleur principale (ou blanc 3 pour
  arc-en-ciel).
- Rendu mkII (remplace le blanc/vert provisoire de T-205) :
  - pad vide = 0 ;
  - cue présente = couleur **atténuée**, canal 0 ;
  - cue en cours = couleur **pleine** canal 0, puis couleur atténuée en
    secondaire sur le canal 9 (**pulsation 1/4**) ; flash tenu = canal 13
    (clignotement 1/8) ;
  - Scene Launch : page courante = blanc 3, autres pages existantes = gris 1,
    pages 6–10 : blanc pulsé.
- **Horloge vers l'APC** : le protocole indique que pulsation/clignotement se
  calent sur le tempo ; envoyer l'horloge MIDI (`F8`, 24 par noire) au mkII
  depuis le tempo T-150 pour que les pads pulsent en rythme (voir T-207).
  **À vérifier sur l'APC de l'utilisateur** ; si sans effet, piloter la
  pulsation nous-mêmes (canal 0, deux couleurs alternées à chaque temps).
- Option « Couleurs des cues sur les pads » (sinon vert/jaune comme l'APC40
  d'origine).

## Modèle de données
`const MK2_PALETTE: [[u8; 3]; 128]` ; option `DeviceSettings.rgb_cue_colors: bool` (défaut `true`) dans `devices.json`.

## Interface
Case « Couleurs des cues sur les pads (mkII) » dans l'onglet Contrôleur.

## Critères d'acceptation
- [ ] Une page de cues vertes/rouges/bleues s'affiche en vert/rouge/bleu (atténués) sur les pads.
- [ ] La cue en cours est plus lumineuse et pulse.
- [ ] Changer la couleur de la cue en direct (T-140) met à jour le pad en < 100 ms.
- [ ] Option décochée → schéma vert/jaune.

## Tests
- Unitaires : `nearest_velocity` sur les couleurs de `presets.rs` (vert `[0,255,0]` → 21, rouge → 5 ou 72, bleu `[0,80,255]` → famille 41–45, blanc → 3) ; variante atténuée ; octets envoyés pour une cue en cours (Note On canal 0 + Note On canal 9).
- Palette : 128 entrées, index 0 = noir.

## Notes
- La palette est une donnée factuelle du protocole Akai (pas un contenu créatif) : pas d'entrée dans `docs/CONTENT_SOURCES.md` nécessaire, mais citer la source en commentaire.
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle).

## Journal
