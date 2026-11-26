---
id: T-258
title: Liste de contrôle avant show
status: todo
area: safety
priority: P1
depends_on: [T-250, T-259]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#5-what-professional-software-and-hardware-offer
---

## Contexte
Les bonnes pratiques (IEC 60825-3, ILDA, formulaire DGTA) reposent sur des vérifications faites avant chaque show : clé et arrêt d'urgence matériel, zones, mesures, autorisations. Une liste à cocher avant le premier armement sur un vrai laser évite les oublis et laisse une trace.

## À faire
- Écran « Avant le show » : liste d'items cochables. Verrou `checklist` (T-250) : non satisfait tant que la liste de la session n'est pas terminée **si une sortie réelle est configurée** (`--device`). En aperçu, pas de verrou.
- Items par défaut (modifiables, dans `<data-dir>/checklist.json`) :
  1. Projecteur classé et étiqueté (IEC/EN 60825-1), interrupteur à clé et arrêt d'urgence matériel testés.
  2. Arrêt d'urgence logiciel (Échap) testé : sortie coupée.
  3. Zones de sécurité et horizon vérifiés sur place, en mire, à faible puissance.
  4. Plafonds de puissance de la sortie vérifiés.
  5. Aucun faisceau vers des miroirs, vitres, surfaces réfléchissantes non prévues.
  6. Faisceaux à au moins 3 m au-dessus du sol public (bonne pratique ; exigée en France) et hors des zones de passage.
  7. Opérateur formé présent pendant tout le show, avec accès à l'arrêt d'urgence.
  8. (extérieur) Autorisation DGTA obtenue, référence saisie ; personne joignable sur GSM ; angles déclarés configurés (T-262).
  9. (balayage public) Mesure d'irradiance au point le plus proche du public ; lentille de divergence / atténuation matérielle ; anti-défaut de balayage matériel.
  10. Signalisation / annonce au public si requis par le lieu.
- Les items 8 et 9 ne s'affichent que si le mode extérieur / balayage public est actif.
- Validité : la session en cours ; redémarrer ou changer de profil (T-260) remet la liste à zéro.
- Chaque validation est journalisée (T-259) avec l'horodatage et le nom saisi de l'opérateur.

## Modèle de données
```rust
#[serde(default)]
pub struct ChecklistItem { pub id: String, pub text_fr: String, pub when: ItemWhen /*Always|Outdoor|AudienceScan*/, pub needs_value: bool }
pub struct ChecklistRun { pub operator: String, pub done: Vec<(String, SystemTime, Option<String>)> }
```

## Interface
Onglet « Avant le show » ; champ « Opérateur » ; items cochables (certains avec un champ de saisie, ex. référence d'autorisation) ; bouton « Liste terminée ». Le bouton Armer affiche « Liste de contrôle à faire » tant que ce n'est pas fait.

## Critères d'acceptation
- [ ] Avec `--device` configuré (test : option de simulation, jamais un vrai device), armement refusé tant que la liste n'est pas terminée
- [ ] En aperçu pur, pas de verrou
- [ ] Liste personnalisée conservée après redémarrage, cases remises à zéro
- [ ] Entrée de journal avec opérateur et items

## Tests
Unitaires sur le verrou et la remise à zéro ; e2e du parcours complet (aperçu + drapeau de simulation de sortie).

## Notes
- Ne pas présenter la liste comme une conformité légale : libellé « aide-mémoire ».
- CLAUDE.md : tests jamais avec `--device` ; utiliser une sortie factice pour simuler « sortie réelle configurée ».

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
