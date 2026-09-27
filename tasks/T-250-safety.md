---
id: T-250
title: Verrous d'armement (interlocks) et raisons de désarmement
status: todo
area: safety
priority: P0
depends_on: []
owner: ""
branch: ""
source: docs/research/safety-regulation.md#7-proposed-features
---

## Contexte
Aujourd'hui l'armement est un simple booléen (`POST /api/arm {on}`) : on ne sait pas qui a armé, pourquoi le laser s'est désarmé, ni ce qui empêche d'armer. Toutes les protections suivantes (arrêt d'urgence, présence opérateur, chien de garde, liste de contrôle, mode public) ont besoin d'un point central unique qui décide si la sortie peut émettre.

## À faire
- Nouveau module `studio/src/interlock.rs` : une « porte » (`ArmGate`) qui est **le seul endroit** où l'état armé change.
- Armer devient une *demande* : elle réussit seulement si **tous** les verrous actifs sont satisfaits ; sinon elle est refusée avec la liste des verrous bloquants.
- Chaque désarmement porte une **raison** et une **source** (interface, clavier, MIDI, API, système).
- Si un verrou passe à « non satisfait » pendant que le laser est armé, la porte désarme immédiatement (raison = ce verrou).
- Dernière étape du pipeline, après la sécurité de T-003 : si la porte n'est pas armée, la trame envoyée à la sortie est entièrement éteinte (l'aperçu continue d'afficher la trame calculée, avec un voyant « Désarmé »).
- Au démarrage : toujours désarmé, raison `Démarrage`.
- API : `GET /api/arm` renvoie `{armed, since, source, last_disarm: {reason, source, at}, blocking: [..]}` ; `POST /api/arm {on:true}` renvoie 409 + `blocking` si refusé. `{on:false}` reste toujours accepté.
- Les verrous sont enregistrés par les autres tâches (T-251, T-252, T-253, T-258, T-255, T-262) ; ici on fournit le mécanisme et un verrou de test.

## Modèle de données
```rust
pub enum ArmSource { Ui, Keyboard, Midi, Api, System }
pub enum DisarmReason { Startup, User, EStop, UiLost, EngineStall, Interlock(String), ProfileChange, Shutdown }
pub struct Interlock { pub id: &'static str, pub label_fr: String, pub ok: bool }
pub struct ArmGate {
    armed: bool,
    since: Option<Instant>,
    source: Option<ArmSource>,
    last_disarm: Option<(DisarmReason, ArmSource, SystemTime)>,
    interlocks: Vec<Interlock>,
}
impl ArmGate {
    pub fn request_arm(&mut self, src: ArmSource) -> Result<(), Vec<String>>;
    pub fn disarm(&mut self, reason: DisarmReason, src: ArmSource);
    pub fn set_interlock(&mut self, id: &'static str, ok: bool); // désarme si armé et !ok
    pub fn gate(&self, frame: &mut [Point]);                     // éteint tout si désarmé
}
```
`Shared.armed` est remplacé par `Shared.gate: ArmGate` (garder le champ `armed` dans `/api/state` pour la compatibilité).

## Interface
- Bouton « Armer » : si refusé, bulle rouge « Armement impossible : » + liste des verrous (libellés français).
- Sous le bouton : « Désarmé — raison : Échap (clavier), 21:42:10 » ou « Armé depuis 3 min 12 s (Espace) ».

## Critères d'acceptation
- [ ] Au démarrage `armed=false`, raison `Démarrage`
- [ ] Un verrou non satisfait fait refuser l'armement (409) avec son libellé
- [ ] Un verrou qui tombe pendant l'armement désarme dans le même tick moteur
- [ ] Désarmé : `/api/frame` de sortie ne contient aucun point allumé ; l'aperçu reste visible
- [ ] `{on:false}` est toujours accepté, même si le corps contient d'autres champs
- [ ] Échap et Espace se comportent comme avant

## Tests
- Unitaires : table de transitions (armé/désarmé × verrous), raisons, `gate()` éteint toutes les couleurs.
- e2e : armer via l'UI (aperçu seulement, pas de `--device`) avec un verrou de test activé par une option de ligne de commande de test → message de refus visible.

## Notes
- Aucune autre partie du code ne doit modifier l'état armé directement : grep `armed =` dans la review.
- CLAUDE.md : laser désarmé au départ, Échap = blackout instantané, tests jamais avec `--device`.
- Liens : T-003 (zones, appliquées avant la porte), T-208 (MIDI utilisera `ArmSource::Midi`).

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
