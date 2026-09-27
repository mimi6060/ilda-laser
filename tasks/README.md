# Tâches — Laser Studio

Ce dossier est le tableau de travail de l'équipe d'agents, et ce que tu
relis. **Un fichier = une tâche.** Les agents de recherche écrivent les
tâches ; les agents de développement les prennent, les réalisent et les
marquent faites ; les reviewers et testeurs les valident.

## Cycle de vie

Chaque fichier commence par un en-tête :

```yaml
---
id: T-042                 # unique, jamais réutilisé
title: Panneau de modificateurs en direct
status: todo              # todo | in-progress | review | done | blocked
area: live                # live | cues | timeline | tempo | midi | output | safety | ilda | ui | qa | infra
priority: P1              # P0 (bloquant) … P3 (plus tard)
depends_on: [T-040]       # ids des tâches à terminer avant
owner: ""                 # agent ou personne qui l'a prise
branch: ""                # feat/<nom> quand elle est commencée
source: docs/research/pro-live-operation.md#live-modifiers
---
```

Puis les sections, toujours dans cet ordre :

1. **Contexte** — pourquoi, pour l'utilisateur (1 à 5 lignes).
2. **À faire** — le comportement attendu, précis.
3. **Modèle de données** — structs/champs Rust, valeurs par défaut.
4. **Interface** — contrôles, libellés en français, raccourcis, MIDI.
5. **Critères d'acceptation** — liste à cocher, testable.
6. **Tests** — unitaires et e2e attendus.
7. **Notes** — pièges, sécurité, propriété intellectuelle, liens.
8. **Journal** — ajouté par les agents : qui a fait quoi, quand, résultat
   des tests, verdict de review.

## Règles

- Un agent de développement ne prend qu'une tâche `todo` dont toutes les
  `depends_on` sont `done`. Il passe le statut à `in-progress`, remplit
  `owner` et `branch`, et le commit sur sa branche.
- Quand c'est prêt : `status: review`, et une entrée dans le Journal.
- Le reviewer passe à `done` (après merge dans `develop`) ou remet
  `in-progress` avec la liste des corrections dans le Journal.
- `INDEX.md` est la vue d'ensemble : une ligne par tâche. Il est **généré**
  (`python3 tasks/make_index.py`) : ne jamais l'éditer à la main, pour que
  des agents en parallèle ne se marchent pas dessus.
- Plages d'identifiants, pour éviter les doublons entre agents :
  T-001–T-099 feuille de route de base · T-100–T-139 looks festival et
  cues évolutifs · T-140–T-199 modificateurs en direct, calques, timeline,
  tempo · T-200–T-229 MIDI / APC40 · T-230–T-249 analyse audio ·
  T-250–T-269 sécurité et réglementation · T-270–T-289 visualiseur et
  ergonomie · T-290+ nouvelles tâches (QA, bugs).
- Les règles de `CLAUDE.md` (sécurité laser, propriété intellectuelle)
  s'appliquent à chaque tâche.

Modèle vide : `_TEMPLATE.md`.
