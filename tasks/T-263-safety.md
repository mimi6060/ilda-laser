---
id: T-263
title: Fiche sécurité du show exportable
status: todo
area: safety
priority: P3
depends_on: [T-254, T-258, T-262]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#34-outdoor-dgta-authorisation-confirmed-primary-sources
---

## Contexte
La demande d'autorisation DGTA (extérieur) et l'analyse de risques d'une salle demandent les mêmes informations : projecteurs, classes, puissances, divergence, longueurs d'onde, directions des faisceaux, mesures de sécurité, opérateur. Laser Studio les a déjà : il peut produire une fiche imprimable à joindre au dossier.

## À faire
- `GET /api/safety/sheet` → page HTML autonome imprimable (CSS d'impression, pas de script), en français.
- Contenu :
  1. Événement : nom, lieu, dates/heures (saisis), opérateur, téléphone portable de la personne présente (saisi, non enregistré dans le journal).
  2. Par sortie : fiche projecteur (T-254) — classe, puissance max par couleur (W), longueurs d'onde (nm), diamètre (cm), divergence (mrad), mode « onde entretenue » ; plafond configuré ; montage et hauteur.
  3. Extérieur (T-262) : coordonnées WGS84, secteurs au format « faisceau n° / angle horizontal de–à / angle vertical de–à », limite de verticale, référence d'autorisation.
  4. Mesures de sécurité actives : zones et horizon (image de l'aperçu avec zones, rendue côté serveur en SVG), garde anti-point fixe, limiteur de strobe, arrêt d'urgence logiciel, présence opérateur, « Ciel coupé », liste de contrôle (dernière exécution).
  5. Estimation d'exposition (T-257) si remplie, avec la mention « estimation ».
  6. Pied de page : « Document généré par Laser Studio. Ne remplace ni une mesure ni l'avis d'un expert. »
- Bouton « Fiche sécurité » qui ouvre la page dans un nouvel onglet (impression → PDF par le navigateur).

## Modèle de données
`EventInfo { name, venue, dates: Vec<(String,String,String)>, operator, phone }` saisi dans le formulaire de la fiche, sauvegardé dans le profil (T-260) sauf `phone`.

## Interface
Onglet Sécurité → « Fiche sécurité » : formulaire Événement puis « Générer ».

## Critères d'acceptation
- [ ] La fiche contient toutes les rubriques techniques du formulaire DGTA (classe, puissance, diamètre, divergence, longueur d'onde, angles)
- [ ] Le SVG montre les zones Blank/Dim/Public
- [ ] Aucun script dans la page générée ; impression propre en A4
- [ ] Le numéro de téléphone n'est écrit dans aucun fichier

## Tests
Unitaires : génération HTML à partir d'un état de test (instantané texte) ; e2e : ouvrir `/api/safety/sheet` et vérifier les titres.

## Notes
- Ne pas reproduire le formulaire officiel lui-même (mise en page, logos) : on fournit une annexe technique. Pas d'usurpation de document officiel.
- CLAUDE.md : tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
