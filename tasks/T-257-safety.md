---
id: T-257
title: Estimateur d'exposition (EMP) et distance de danger (DNRO)
status: todo
area: safety
priority: P2
depends_on: [T-254]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#22-mpe-for-audience-exposure
---

## Contexte
Avant un show, l'opérateur doit savoir de combien son projecteur dépasse l'exposition maximale permise (EMP) à la distance du public, et jusqu'où un faisceau fixe reste dangereux (DNRO, distance nominale de risque oculaire). Un calculateur intégré, alimenté par la fiche projecteur, rend les ordres de grandeur visibles. Ce n'est jamais une mesure.

## À faire
- Module `studio/src/mpe.rs`, fonctions pures :
  - diamètre du faisceau à la distance d : `D(d) = a + d·φ` (a = diamètre de sortie, φ = divergence en rad) ;
  - irradiance moyenne à d pour une puissance P : `E = 4P / (π·D²)` ; moyennée sur l'ouverture de mesure de 7 mm si `D` < 7 mm ;
  - EMP visible (400–700 nm, 18 µs ≤ t ≤ 10 s) : `H = 18·t^0,75 J/m²`, `E_EMP = H/t` ; t = 0,25 s → ≈ 2,55 mW/cm² ;
  - valeurs de référence ILDA affichées à côté : 2,5 (fixe), 10 (balayage continu) mW/cm² ;
  - DNRO : distance où `E = E_EMP` ;
  - facteur de dépassement à la distance du public, avec l'atténuation de zone (T-003 Dim) et le plafond de sortie (T-254) appliqués.
- Entrées : fiche projecteur (T-254), distance minimale au public (m), atténuation de zone. Somme des couleurs à pleine puissance (cas le plus défavorable).
- Page de résultats dans l'onglet Sécurité ; aucun effet sur la sortie.

## Modèle de données
```rust
pub struct ExposureInput { pub power_mw: f32, pub aperture_mm: f32, pub divergence_mrad: f32, pub distance_m: f32, pub attenuation: f32, pub exposure_s: f32 /*0.25*/ }
pub struct ExposureResult { pub irradiance_mw_cm2: f32, pub mpe_mw_cm2: f32, pub ratio: f32, pub nohd_m: f32 }
```

## Interface
Onglet Sécurité → « Estimation d'exposition » : champs « Distance minimale du public (m) », « Atténuation dans la zone public (%) » ; résultats « Irradiance estimée », « EMP (0,25 s) », « Dépassement × N », « Distance de danger (DNRO) ». Bandeau fixe : « Estimation théorique — ne remplace pas une mesure avec un appareil étalonné ».

## Critères d'acceptation
- [ ] EMP à 0,25 s = 2,55 mW/cm² ± 1 %
- [ ] 1 W, 5 mm, 1 mrad, 10 m → irradiance et DNRO conformes à un calcul de référence écrit dans le test
- [ ] Résultat marqué rouge si ratio > 1, vert sinon
- [ ] Aucun changement de sortie quand on modifie les champs

## Tests
Unitaires avec valeurs calculées à la main (dans les commentaires du test).

## Notes
- Formules simplifiées (faisceau gaussien à profil uniforme, t = 0,25 s, source ponctuelle, une seule impulsion). Pour les effets balayés, l'ILDA demande d'évaluer impulsion unique, impulsions répétées et puissance moyenne : hors périmètre, le dire dans l'interface.
- Sources : IEC 60825-1, ILDA (liens dans le rapport).
- CLAUDE.md : aucune sortie réelle.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
