---
id: T-203
title: Apprentissage MIDI (clic droit → « Apprendre MIDI »)
status: todo
area: midi
priority: P1
depends_on: [T-202, T-145, T-209]
owner: ""
branch: ""
source: docs/research/midi-apc40.md#6-midi-learn-and-mapping-storage
---

## Contexte
Pour affecter n'importe quel bouton/potard/fader à n'importe quel contrôle
sans éditer de JSON : on désigne le contrôle à l'écran, on touche le matériel,
c'est enregistré. C'est aussi ce qui rend utilisables les contrôleurs autres
que l'APC40.

## À faire
- Tous les contrôles mappables de `index.html` portent `data-control="<id>"`
  (identifiants T-145) ; les boutons de cue portent `data-control` de cue.
- **Mode apprentissage** : bouton « Apprendre MIDI » dans l'onglet Contrôleur.
  Actif → les contrôles mappables sont entourés en pointillés verts ; ceux
  déjà mappés affichent une pastille avec le message (« CC 14 », « Note 82 »).
  Un clic sur un contrôle le sélectionne comme cible.
- **Clic droit** sur un contrôle mappable, à tout moment → menu : « Apprendre
  MIDI », « Oublier MIDI », « Apprendre avec Shift ».
- Backend : `POST /api/midi/learn { target, args, shift }` arme
  l'apprentissage (délai 15 s) ; le **prochain** Note On ou CC reçu (hors
  Shift, hors messages temps réel, hors CC dont la valeur ne bouge pas) crée
  le mapping dans le profil utilisateur du port (copie `-perso` si le profil
  était intégré, T-201) et l'enregistre sur disque.
  `POST /api/midi/learn/cancel`, `POST /api/midi/mapping/delete { port, index }`.
- Mode déduit automatiquement : Note → `trigger` (ou `toggle`/`momentary`
  selon le type de la cible déclaré par T-145) ; CC d'un contrôle connu comme
  encodeur dans le profil (`0x2F`, `0x0D` sur APC) → `relative` ; autre CC →
  `absolute` avec les bornes de la cible et `pickup: true`.
- Si le message est déjà mappé : l'UI demande « Remplacer l'ancienne
  affectation (Taille) ? » — Oui / Non.
- Échap annule l'apprentissage (et reste un blackout : Échap fait les deux).
- Liste des affectations dans l'onglet Contrôleur : message, cible (libellé
  français), mode, Shift, bouton « Supprimer ».

## Modèle de données
`Shared.midi.learn: Option<LearnRequest { target: String, args: Value, shift: bool, until: Instant }>`.
`/api/midi` expose `learn` (cible en attente) et `learned` (dernier mapping créé) pour que l'UI se mette à jour.

## Interface
- Bouton « Apprendre MIDI » / « Terminer » (onglet Contrôleur).
- Bandeau pendant l'attente : « Touchez un bouton, un potard ou un fader de votre contrôleur… (Échap pour annuler) ».
- Menu contextuel : « Apprendre MIDI », « Apprendre avec Shift », « Oublier MIDI ».
- Pas de raccourci à une lettre (toutes les lettres sont des touches de cues AZERTY).

## Critères d'acceptation
- [ ] Clic droit sur le curseur « Taille » → « Apprendre MIDI » → tourner un potard : le potard pilote la taille, l'affectation apparaît dans la liste.
- [ ] Le mapping survit au redémarrage (fichier dans `<data-dir>/midi/profiles/`).
- [ ] Apprendre sur un profil intégré crée `apc40-mk2-perso` sans modifier le profil intégré.
- [ ] « Oublier MIDI » supprime l'affectation.
- [ ] Sans message dans les 15 s, l'apprentissage s'annule avec un message.

## Tests
- Unitaires : déduction du mode (note, CC absolu, encodeur connu), remplacement d'un doublon, expiration, création de la copie `-perso`.
- e2e (Playwright, studio lancé avec `--no-midi --midi-test`, voir T-209) : clic droit → Apprendre → injection d'un CC via `/api/midi/inject` → le curseur bouge et la liste contient l'affectation.

## Notes
- Inspiration : « Learn mode » de MadMapper (clic droit → Add Control), Teach-In de Showcontroller (sans retour LED pour les contrôleurs non-APC).
- Règles de CLAUDE.md (sécurité laser, propriété intellectuelle). L'armement du laser ne peut pas être appris sans l'option de T-208.

## Journal
