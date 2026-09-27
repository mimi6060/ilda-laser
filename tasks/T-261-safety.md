---
id: T-261
title: Verrouillage des réglages de sécurité par code
status: todo
area: safety
priority: P3
depends_on: [T-260, T-259]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#7-proposed-features
---

## Contexte
Pendant un show, un invité (VJ, DJ) peut utiliser l'interface pour lancer des cues. Il ne doit pas pouvoir élargir une zone, monter un plafond ou activer le balayage public par erreur. Le responsable verrouille les réglages de sécurité avec un code.

## À faire
- Réglage « Verrouiller la sécurité » avec un code (4 à 8 chiffres). Stocker un hachage (argon2 ou `sha256` salé) dans `<data-dir>/safety-lock.json`, jamais le code en clair.
- Verrouillé : toutes les routes qui **modifient** des réglages de sécurité (zones, plafonds, profils, présence, garde, mode public, extérieur, liste de contrôle) renvoient 423 ; ce qui **réduit** le risque reste permis (arrêt d'urgence, désarmer, baisser la luminosité, « Ciel coupé »).
- Déverrouiller : code ; 5 échecs → attente de 60 s. Reverrouillage automatique après 10 min.
- Oubli du code : supprimer le fichier à la main (documenté), l'application démarre alors déverrouillée et le journalise.
- Journaliser verrouillage, déverrouillage, échecs (T-259).

## Modèle de données
```rust
#[serde(default)]
pub struct SafetyLock { pub hash: Option<String>, pub salt: Option<String>, pub relock_min: u32 /*10*/ }
```

## Interface
Cadenas dans la section Sécurité : « Verrouiller… » / « Déverrouiller… ». Contrôles de sécurité grisés avec l'infobulle « Verrouillé par le responsable sécurité ».

## Critères d'acceptation
- [ ] Verrouillé : `POST /api/safety` → 423 ; `/api/estop` et désarmement → 200
- [ ] Le code n'apparaît dans aucun fichier ni journal
- [ ] 5 mauvais codes → 60 s d'attente
- [ ] Reverrouillage après 10 min d'inactivité

## Tests
Unitaires sur le hachage, les routes filtrées et la temporisation (horloge simulée) ; e2e : verrouiller, essayer de modifier une zone.

## Notes
- Complète T-283 (« Mode spectacle », verrou d'édition de l'interface sans code) : T-283 protège contre les clics accidentels, T-261 protège les réglages de sécurité côté API avec un code. Si T-283 est actif, T-261 reste indépendant (les deux verrous s'additionnent).
- Protection contre l'erreur, pas contre un attaquant (l'API est locale, CLAUDE.md : localhost uniquement).
- CLAUDE.md : tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
