---
id: T-302
title: Banque SD ShowNET : numéroter les cues et exporter un dossier prêt à copier
status: todo
area: ilda
priority: P2
depends_on: [T-001, T-011]
owner: ""
branch: ""
source: docs/research/shownet-options.md#3-carte-sd--exporter-notre-contenu
---

## Contexte
Le ShowNET sait jouer seul les fichiers `001.ild`–`229.ild` de sa microSD, choisis par DMX/Art-Net. Pour s'en servir avec nos propres looks, il faut savoir quel cue porte quel numéro, et exporter toute la banque d'un coup sur la carte (lecteur de carte du Mac, pas besoin de l'Admin Tool Windows).

## À faire
- Nouveau `studio/src/sdbank.rs` : une banque = liste ordonnée `(numéro 1..=229, cue/scène, durée d'export)`.
- Attribution automatique des numéros libres ; refus de 0 et de 230–255 (réservés) ; doublons refusés.
- Export : rendu de chaque cue en aperçu (moteur hors ligne, durée en secondes à fps fixe) → `nnn.ild` au format 5 via l'écrivain de T-001, avec luminosité, calibration, zones et blanking déjà appliqués. Refus si un fichier dépasse 8 Mo ; avertissement au-delà de 6 Mo.
- Écrit aussi `banque.json` (numéro → nom du cue), relu par T-301.
- API : `GET/POST /api/sdbank`, `POST /api/sdbank/export {dir}` (dossier local choisi par l'utilisateur, jamais dans le dépôt).

## Modèle de données
```rust
#[serde(default)]
pub struct SdBank { pub slots: Vec<SdSlot> }
#[serde(default)]
pub struct SdSlot { pub number: u8, pub cue_id: String, pub seconds: f32 /* 4.0 */ }
```
Sauvegardé dans le projet (ou `studio-data/sdbank.json`).

## Interface
Onglet *Réglages › Carte SD ShowNET* : tableau *N°*, *Cue*, *Durée (s)* ; boutons *Ajouter le cue courant*, *Numéroter automatiquement*, *Exporter vers un dossier…*. Message : « Copiez les fichiers à la racine de la carte microSD ».

## Critères d'acceptation
- [ ] Numéros 0 et 230–255 refusés ; doublon refusé
- [ ] L'export produit `001.ild` … lisibles par notre lecteur ILDA (T-001), en format 5
- [ ] Les points exportés respectent la luminosité max et les zones de sécurité du projet
- [ ] Un `sdbank.json` sans champ `seconds` se charge (valeur par défaut)
- [ ] `banque.json` liste numéro → nom

## Tests
Unitaires : validation des numéros, export dans un dossier temporaire puis relecture, limites de taille. e2e : ajouter deux cues, exporter vers un dossier temporaire, vérifier `/api/sdbank`.

## Notes
- Propriété intellectuelle : on n'exporte que **nos** looks. Ne jamais copier dans le dépôt les jeux SD de Laserworld (Standard/Extended Set) ; pas de convertisseur visant leur contenu.
- Le ShowNET applique ses propres effets par-dessus en mode DMX : nos protections ne valent que pour le contenu exporté.

## Journal
- 2026-10-03 — agent de recherche : tâche créée depuis `docs/research/shownet-options.md`.
