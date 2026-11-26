---
id: T-262
title: Mode extérieur — angles déclarés, « Ciel coupé », limite 45°
status: todo
area: safety
priority: P2
depends_on: [T-003, T-250, T-254, T-259]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#34-outdoor-dgta-authorisation-confirmed-primary-sources
---

## Contexte
En Belgique, projeter un laser dans l'espace aérien exige **toujours** une autorisation de la DGTA (circulaire GDF-12). Le formulaire demande le nombre de faisceaux et, pour chacun, la plage d'angles horizontaux et verticaux ; il exige qu'un faisceau qui touche un aéronef soit **immédiatement dévié**, et limite les faisceaux lumineux à 45° de la verticale. Le logiciel doit faire respecter les angles déclarés et offrir une coupure instantanée du ciel.

## À faire
1. **Montage du projecteur** (par sortie) : angle optique de balayage total (°, depuis la fiche T-254), inclinaison (élévation de l'axe central, °), azimut de l'axe (°, 0 = nord), hauteur au-dessus du sol (m). Conversion point normalisé (x,y) → (azimut, élévation) : `az = azimut_axe + x·angle/2`, `él = inclinaison + y·angle/2` (approximation petits angles, documentée).
2. **Plages déclarées** : liste de secteurs `{az_min, az_max, el_min, el_max}` (comme sur le formulaire). En mode extérieur, tout point allumé dont (az, él) est hors de tous les secteurs est éteint, avec découpe au bord (comme les zones de T-003).
3. **Limite de verticale** : réglage « Écart max à la verticale » (défaut 45°) : un point au-dessus de l'horizon géographique (él > 0) dont l'écart à la verticale (90° − él) dépasse la limite est éteint. Désactivable seulement si l'autorisation le permet (case + référence obligatoire).
4. **« Ciel coupé »** : bascule instantanée qui éteint tous les points d'élévation > 0 sans désarmer le reste (le show au sol continue). Raccourci `C`, bouton dédié, action MIDI `safety.sky_off`. Effet dans le tick suivant. Rétablir = geste explicite (même bouton). Toujours autorisé même si la sécurité est verrouillée (T-261).
5. Verrou `outdoor_auth` (T-250) : en mode extérieur, armement refusé si le champ « Référence d'autorisation DGTA » est vide.
6. Export des secteurs au format du formulaire (tableau faisceau / angle horizontal de–à / angle vertical de–à) pour T-263.

## Modèle de données
```rust
#[serde(default)]
pub struct Mounting { pub scan_angle_deg: f32 /*40*/, pub tilt_deg: f32 /*0*/, pub azimuth_deg: f32 /*0*/, pub height_m: f32 /*0*/ }
#[serde(default)]
pub struct OutdoorSettings {
    pub enabled: bool, pub authorization_ref: String, pub wgs84: Option<(f64, f64)>,
    pub sectors: Vec<Sector>, pub max_from_vertical_deg: f32 /*45*/, pub vertical_limit_waived: bool,
}
pub struct Sector { pub az_min: f32, pub az_max: f32, pub el_min: f32, pub el_max: f32 }
```
`sky_off: bool` est un état de session (non sauvegardé, faux au démarrage).

## Interface
Section « Extérieur » : case « Show en extérieur », « Référence d'autorisation DGTA », coordonnées, montage, secteurs (tableau éditable). Aperçu : secteurs en vert, zone interdite hachurée. Gros bouton « CIEL COUPÉ » (bleu) à côté du bouton d'arrêt quand le mode extérieur est actif.

## Critères d'acceptation
- [ ] Un faisceau hors des secteurs déclarés n'est jamais allumé en sortie
- [ ] Avec la limite 45°, un faisceau à 30° d'élévation (60° de la verticale) est éteint ; à 60° d'élévation il passe
- [ ] « Ciel coupé » : aucun point d'élévation > 0 allumé au tick suivant ; les points au sol restent
- [ ] Mode extérieur sans référence → armement refusé
- [ ] Export des secteurs identique à la saisie

## Tests
Unitaires : conversion d'angles, découpe aux bords des secteurs, limite verticale, « Ciel coupé ». e2e : activer, vérifier `/api/frame` de sortie.

## Notes
- Sources : page DGTA « Skytracers et lasers » et formulaire officiel (liens dans le rapport §3.4). Délai : demande 20 à 60 jours ouvrables avant, 129 € (2026).
- Les limites de l'OACI (zone sans laser 50 nW/cm² près des aéroports) ne sont pas calculables sans position des pistes : afficher un rappel, pas de calcul.
- Ne jamais présenter l'outil comme une garantie de conformité aéronautique.
- CLAUDE.md : tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
