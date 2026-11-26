---
id: T-260
title: Profils de sécurité par lieu
status: todo
area: safety
priority: P2
depends_on: [T-003, T-254, T-101]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#7-proposed-features
---

## Contexte
Les zones, plafonds et limites dépendent du lieu (salon, club, extérieur). Les refaire à chaque fois est source d'erreurs ; charger par mégarde les réglages d'un autre lieu aussi. Un profil nommé regroupe tout ce qui touche à la sécurité et se charge d'un bloc, en désarmant.

## À faire
- Profil = zones et horizon (T-003), plafonds de sortie (T-254), limiteur de strobe et plancher (T-101), réglages de garde (T-256), présence opérateur (T-252), liste de contrôle (T-258), mode extérieur (T-262) quand ces tâches existent (champs `Option`, `#[serde(default)]`), plus : nom, lieu, notes, date de dernière vérification sur place, « balayage public autorisé dans ce lieu » (bool, faux par défaut ; T-255 le consulte).
- Profil intégré **« Aperçu sûr »** (non supprimable, non modifiable) : puissance max 20 %, horizon à 0, balayage public interdit. Chargé au premier lancement.
- Charger un profil : désarme (`ProfileChange`), remet la liste de contrôle à zéro, journalise (T-259).
- Stockage `<data-dir>/safety-profiles/<nom>.json` ; `GET/POST /api/safety/profiles`, `POST /api/safety/profiles/load {name}`.
- Le profil actif est rappelé au démarrage, mais le démarrage reste désarmé.
- Avertissement si la « dernière vérification sur place » a plus de 30 jours.

## Modèle de données
```rust
#[serde(default)]
pub struct SafetyProfile {
    pub name: String, pub venue: String, pub notes: String, pub verified_on: Option<String>,
    pub audience_scan_allowed: bool, // false
    pub safety: SafetySettings,       // T-003
    pub limits: BTreeMap<String, OutputLimits>, // T-254, par sortie
    pub strobe: Option<StrobeSettings>, pub dwell: Option<DwellGuard>, pub presence: Option<PresenceSettings>,
    pub outdoor: Option<OutdoorSettings>, pub checklist: Option<Vec<ChecklistItem>>,
}
```

## Interface
Section Sécurité, en haut : sélecteur « Profil de sécurité », boutons « Enregistrer sous… », « Marquer comme vérifié aujourd'hui ». Le nom du profil est affiché dans la barre d'état à côté de l'état armé.

## Critères d'acceptation
- [ ] Charger un profil pendant l'armement → désarmé, raison « Changement de profil »
- [ ] « Aperçu sûr » ne peut être ni modifié ni supprimé
- [ ] Un profil sans les champs des tâches futures se charge (défauts)
- [ ] Profil actif rechargé au redémarrage, laser désarmé

## Tests
Unitaires : sérialisation aller-retour, compatibilité ascendante (JSON minimal), désarmement au chargement. e2e : créer, recharger, vérifier `/api/safety`.

## Notes
- Une scène/look ne contient jamais de réglages de sécurité : garder la séparation (T-003, `pro-live-operation.md` : « la sécurité n'est jamais un modificateur en direct »).
- CLAUDE.md : laser désarmé au démarrage ; tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
