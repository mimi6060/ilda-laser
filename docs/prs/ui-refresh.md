# feat/ui-refresh — interface façon console lumière (T-299)

## Quoi / pourquoi
L'utilisateur trouvait l'interface « triste ». Nouveau thème inspiré de
l'esprit des consoles lumière (idées seulement, design maison) :
- jetons de couleur revus (fond profond, accent cyan, dégradé « laser »
  cyan → violet → magenta), cartes arrondies avec ombre, barres de défilement fines ;
- logo SVG maison + nom en dégradé, liseré laser sous la barre du haut ;
- onglets LIVE / TIMELINE / CRÉATION / RÉGLAGES avec une couleur chacun ;
- tempo en afficheur LCD, LEDs de temps qui brillent (le « 1 » en magenta) ;
- bouton LASER qui pulse quand il émet, halo rouge autour de l'aperçu en émission ;
- curseurs à piste éclairée (suivent aussi les changements serveur/MIDI) ;
  calques en tranches de console ;
- cues : tuiles avec **miniature** du look + couleur par page, cue actif
  qui brille avec une barre animée.

Serveur : `presets::thumbnail()` rend chaque cue (Animator neuf, 0,5 s,
un peu de musique) et l'emballe en hex (5 octets/point, ≤160 points).
`GET /api/presets/thumbs` sert un cache (re-rendu seulement si le look
change, ex. figure modifiée), réchauffé au démarrage dans un thread à part.

## Tests
- `cargo test` : 695 + 2 + 2 ; nouveau test : chaque cue a une miniature non noire.
- clippy `-D warnings` propre.
- e2e : 181 + 2 nouveaux (`theme.spec.ts`) verts.

## Risques
- Trouvé en route : rendre les miniatures sur le fil HTTP unique retardait
  la fin de `presence.spec.ts › two pages…` (désarmement à la fermeture).
  Corrigé par le cache + le calcul au démarrage hors du fil HTTP.
- Affichage seulement : les miniatures ne passent jamais par la sortie,
  la chaîne de sécurité est inchangée.
- `color-mix()` demande un navigateur récent (Chrome 111+, Safari 16.2+).

## Review
Revue par l'intégrateur sur le résultat fusionné : ids/classes utilisés par le JS et les e2e inchangés, aucun chemin de sortie touché, IP OK (logo et thème maison).

Verdict: APPROVED
