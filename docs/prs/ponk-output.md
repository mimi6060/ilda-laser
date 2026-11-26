# feat/ponk-output — Sortie PONK vers MadMapper/MadLaser (T-300)

## Quoi / pourquoi
Le protocole du ShowNET est chiffré et le SDK Laserworld n'arrive pas.
MadMapper/MadLaser (licencié par Laserworld, déjà utilisé par l'utilisateur
sur son ShowNET) reçoit des tracés d'autres logiciels par **PONK**, protocole
UDP ouvert (Apache-2.0). Cette branche ajoute une `Output` PONK : Laser
Studio fait le rendu, MadMapper sert de pont officiel. Aucun contact avec le
ShowNET ni avec MadMapper au-delà du protocole public.

- `studio/src/ponk.rs` (nouveau) : `PonkOutput` implémente `Output`.
  - Format `XY_F32_RGB_U8`, datagrammes ≤ 8192 octets (en-tête compris),
    CRC, n° de frame, nom « Laser Studio », id d'émetteur stable dans
    `<data-dir>/ponk.json`.
  - Un chemin PONK par suite de points allumés ; les points éteints (et
    toute coordonnée non finie) coupent le chemin et ne partent pas.
    Coordonnées bornées à -1..1, couleurs 0..1 → u8.
  - Métadonnées par chemin : `PATHNUMB` (index) et `PRESRVOR=1` (garder
    notre ordre). `ANGLEOPT`/`MINIPNTS`/`MAXSPEED` ne sont pas envoyés :
    ils restent aux réglages de la surface MadMapper (documenté).
  - **Sécurité** : chaque tick envoie une frame ; désarmé, elle est vide
    (l'encodeur émet un chemin à 0 point, jamais un en-tête seul). Double
    garde : l'`OutputStage` blanke déjà la trame, et la sortie ignore en
    plus les points tant qu'elle n'est pas armée. `blank_now` envoie une
    frame vide tout de suite. `kill_switch` (arrêt d'urgence, sans le
    thread moteur) désarme et envoie une frame vide immédiatement depuis le
    thread qui le déclenche (socket partagé via `Arc`). Arrêt propre :
    désarmement + 3 frames vides (chemin existant de l'`OutputStage`). Une
    frame impossible à encoder (> 65535 points) part comme frame vide.
  - Socket non bloquant : le moteur n'attend jamais le réseau. Erreurs
    d'envoi journalisées une fois (jusqu'à changement).
  - Les trames reçues sont celles du pipeline existant : calibration
    (bornée -1..1) → `safety::apply` (strobe, horizon, zones) → porte
    d'armement/e-stop/hold-to-run.
- `studio/src/output.rs` : `Output::kind()` (défaut `"dac"`, `"test"` pour
  `FileLogOutput`, `"ponk"`), pour l'UI.
- `studio/src/main.rs` : `--ponk [HOST:PORT]`, désactivé par défaut. Seul :
  `127.0.0.1:5583` (MadMapper sur ce Mac, unicast). Exclusif avec
  `--device`, `--test-output` et `--midi-test` (refus au parsing).
  `Shared::output_kind`.
- `studio/src/web.rs` : `output_kind` dans `/api/state` et `/api/frame`.
- `studio/src/index.html` : pastille de sortie dans l'en-tête (« PONK »,
  « DAC », « TEST », « APERÇU », « ERREUR »), avec les jetons du bloc
  Theme (T-299) ; le texte reste « Sortie : PONK → MadMapper (<cible>) » ;
  info-bulle en français.
- `docs/ponk-madmapper.md` : guide utilisateur (français) de mise en place
  dans MadMapper.

### Écart avec la fiche T-300
La fiche proposait `--ponk` = multicast 239.255.10.24 par défaut +
`--ponk-target` pour l'unicast. J'ai fait un seul `--ponk [HOST:PORT]` qui
vise par défaut **127.0.0.1:5583 en unicast** (le spec PONK dit qu'un
récepteur abonné au multicast reçoit aussi l'unicast sur son port) : par
défaut rien ne sort de la machine. Le multicast reste possible en le
demandant explicitement (`--ponk 239.255.10.24:5583`, TTL 1).

## Dépendance tierce
- **`ponk-protocol` 0.1.0** — <https://github.com/ModulaserApp/ponk-protocol>,
  licence **MIT** (Copyright 2026 RS Product Studio B.V.), zéro dépendance,
  MSRV 1.88 (on est en 1.98). Vérifiée le 2026-10-03 : code lu (encodeur,
  décodeur, réassembleur borné, refus des coordonnées non finies). Aucun
  fichier copié dans le dépôt, seulement une dépendance Cargo.
- Spec PONK : <https://github.com/madmappersoftware/Ponk>, Apache-2.0 (lue,
  rien copié).

## Tests
- `cargo build -p laser-studio` : OK.
- `cargo test -p laser-studio` : 704 passed, 7 ignored (+ 2 + 2 tests
  d'intégration) ; dont 8 nouveaux dans `ponk::tests` et 1 dans `main`.
  - encodage → décodage (crate) = mêmes chemins, découpage aux points
    éteints, métadonnées ;
  - trame sombre = trame vide, toujours un datagramme ;
  - hors champ / NaN jamais envoyés ;
  - trame de 3000 points découpée en datagrammes ≤ 8192 et réassemblée
    identique (`PonkAssembler`) ;
  - id d'émetteur stable ;
  - réseau réel **en unicast vers 127.0.0.1, port choisi par l'OS** (jamais
    5583, jamais de multicast) : via l'`OutputStage`, désarmé = frames
    vides à chaque tick, armé = la trame, hold-to-run relâché = vide,
    kill switch = frame vide immédiate puis vides, arrêt = 3 frames vides ;
    sortie désarmée appelée directement = vide ;
  - CLI : désactivé par défaut, `--ponk` seul, avec adresse, refus avec
    `--device` / `--test-output` / `--midi-test` ; `no_startup_path_is_armed`
    couvre `--ponk`.
- `cargo clippy -p laser-studio --all-targets -- -D warnings` : propre.
- e2e : nouveau `tests/ponk.spec.ts` (3 tests) — le spec ouvre son propre
  récepteur UDP sur 127.0.0.1 (port libre), lance le studio avec
  `--ponk 127.0.0.1:<port>` (le harnais refuse 5583 et n'accepte comme
  sortie que cette cible), vérifie la pastille, les frames vides en continu
  désarmé, les points armé, et les frames vides juste après Échap.
  Suite complète (`npx playwright test --workers=2`, machine chargée) :
  186 tests, tous verts — `presence.spec.ts:72` (« two pages open, one
  closed », sans lien avec la sortie) a échoué une fois sous charge puis est
  passé relancé seul ; la fin de `workspaces.spec.ts` (8 tests, interrompue
  par la session) a été relancée seule : 13/13 verts.

## Risques / questions ouvertes
- **Échelle et sens des coordonnées** dans MadMapper non vérifiés (le spec
  ne les fixe pas) : on envoie -1..1, y vers le haut comme l'aperçu. À
  vérifier par l'utilisateur, sortie laser MadMapper **coupée** (guide §2).
- Le nom exact de la source dans MadMapper (« Live Source » dans la liste
  des médias, d'après le forum garageCube) n'a pas pu être vérifié ici.
- Le forum signale que MadMapper lit aussi sur 127.0.0.1 ; si l'unicast
  local ne marche pas chez l'utilisateur, utiliser l'IP réseau du Mac ou le
  multicast (documenté).
- Si le processus est tué brutalement (SIGKILL), aucune frame vide ne
  part : MadMapper garde la dernière image. Documenté (garder la main sur
  la sortie laser de MadMapper). Un battement côté récepteur n'existe pas
  dans PONK.
- MadMapper re-rend les chemins (vitesse, angles) : notre densification et
  nos temps de pause aux coins ne sont pas transmis tels quels.

## Review
