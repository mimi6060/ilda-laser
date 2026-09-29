---
id: T-254
title: Plafonds de puissance par sortie et fiche projecteur
status: in-progress
area: safety
priority: P0
depends_on: [T-250]
owner: "dev-agent (power-caps)"
branch: feat/power-caps
source: docs/research/safety-regulation.md#7-proposed-features
---

## Contexte
La luminosité est aujourd'hui un réglage de look (0..1) que les cues, LFO et le MIDI peuvent pousser à 1,0. T-208 parle d'un « maximum de sécurité » qui n'existe pas encore. Il faut un plafond matériel par sortie, que rien en amont ne peut dépasser, et connaître le projecteur (classe, puissance) pour les calculs et la fiche de sécurité.

## À faire
- Par sortie : `max_power` global (0..1, défaut 0,5) et `max_color: [f32;3]` (défaut 1,0 chacun), appliqués **après** T-003 et avant la porte d'armement : `r = min(r, max_color[0]) * max_power`, etc. (multiplication pour le global, écrêtage pour les couleurs).
- Ce plafond est distinct de `color_gain` de T-003 (équilibrage des couleurs) : lui est une limite de sécurité, jamais modifiable depuis le MIDI, les cues, la timeline ou l'API de look.
- Fiche projecteur (informative, utilisée par T-257 et T-263) : nom, classe IEC 60825-1, puissance max par couleur (mW), longueurs d'onde (nm), diamètre de sortie (mm), divergence (mrad), angle de balayage optique (°), présence d'un système anti-défaut de balayage matériel (oui/non/inconnu), lentille de divergence (oui/non).
- `max_brightness_effective` exposé à T-208 : `max_power` de la sortie active (remplace le 1,0 provisoire).
- Sauvegarde `<data-dir>/outputs.json`, `GET/POST /api/outputs/limits`. Changer un plafond vers le haut pendant l'armement est refusé (409) ; vers le bas est appliqué immédiatement.

## Modèle de données
```rust
#[serde(default)]
pub struct OutputLimits { pub max_power: f32 /*0.5*/, pub max_color: [f32; 3] /*[1.0;3]*/ }
#[serde(default)]
pub struct ProjectorInfo {
    pub name: String, pub class: String /*"4"*/, pub power_mw: [f32; 3], pub wavelength_nm: [u16; 3],
    pub aperture_mm: f32, pub divergence_mrad: f32, pub scan_angle_deg: f32 /*40.0*/,
    pub hw_scan_fail: Option<bool>, pub divergence_lens: bool,
}
```

## Interface
Onglet « Sorties » → encadré « Limites de sécurité » : « Puissance max (%) », « Rouge / Vert / Bleu max (%) », puis « Fiche projecteur ». Le curseur de luminosité principal affiche une graduation à la position du plafond.

## Critères d'acceptation
- [ ] Look à luminosité 1,0 avec `max_power=0,3` → aucune valeur de couleur > 0,3 en sortie
- [ ] `max_color` vert 0,2 → canal vert ≤ 0,2 quel que soit le look
- [ ] Hausse du plafond refusée pendant l'armement ; baisse appliquée au tick suivant
- [ ] Anciennes installations sans `outputs.json` : défauts appliqués, rien ne casse

## Tests
Unitaires sur l'étage de plafonnement ; e2e : régler 30 %, luminosité à 100 %, vérifier `/api/frame` de sortie.

## Notes
- Défaut 50 % volontairement prudent : l'utilisateur monte en connaissance de cause.
- Journaliser chaque changement (T-259).
- CLAUDE.md : sécurité appliquée en dernier ; tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
