# Index des tâches

_Généré par `python3 tasks/make_index.py` — ne pas éditer à la main._

## À faire (107)

| id | tâche | domaine | priorité | dépend de | branche |
|---|---|---|---|---|---|
| [T-003](T-003-safety.md) | Zones de sécurité, horizon, calibration couleur | safety | P0 | [] |  |
| [T-245](T-245-safety.md) | Sécurité de la réactivité audio (limiteur, péremption, silence, pas d'armement) | safety | P0 | [T-237, T-101] |  |
| [T-254](T-254-safety.md) | Plafonds de puissance par sortie et fiche projecteur | safety | P0 | [T-250] |  |
| [T-271](T-271-safety.md) | Barre du haut fixe : noir, armement, maître, tempo, état de sortie | safety | P0 | [T-270] |  |
| [T-001](T-001-ilda.md) | Lecteur/écrivain ILDA maison | ilda | P1 | [] |  |
| [T-002](T-002-output.md) | Optimiseur de points (tracé laser pro) | output | P1 | [] |  |
| [T-103](T-103-cues.md) | Chasers, coups sur le kick et strobes de faisceaux | cues | P1 | [T-100, T-101] |  |
| [T-105](T-105-cues.md) | Tunnels, cônes, soleil et rayons tournants | cues | P1 | [T-100] |  |
| [T-106](T-106-cues.md) | Nappes : liquid sky, lame, rideaux, cascade, scanner, lamelles, aurore, grille | cues | P1 | [T-100] |  |
| [T-110](T-110-cues.md) | Page de cues « Festival » | cues | P1 | [T-102, T-103, T-104, T-105, T-106, T-107, T-130] |  |
| [T-143](T-143-ui.md) | Panneau « Direct » (modificateurs en direct dans l'interface) | ui | P1 | [T-140, T-141, T-150] |  |
| [T-157](T-157-cues.md) | Cues évolutifs : un cue = une mini-timeline (calques internes, courbes sur modificateurs, LFO) | cues | P1 | [T-111, T-151, T-140] |  |
| [T-203](T-203-midi.md) | Apprentissage MIDI (clic droit → « Apprendre MIDI ») | midi | P1 | [T-202, T-145, T-209] |  |
| [T-204](T-204-midi.md) | Profil APC40 par défaut (disposition Laser Studio) pour APC40 et APC40 mkII | midi | P1 | [T-201, T-202, T-140, T-145, T-150, T-155, T-160, T-208, T-209] |  |
| [T-205](T-205-midi.md) | Retour LED sur l'APC40 (cue active, page, calques, battement) | midi | P1 | [T-201, T-204, T-150] |  |
| [T-211](T-211-midi.md) | Contrôleurs MIDI génériques (n'importe quel appareil) | midi | P1 | [T-200, T-201, T-202, T-203] |  |
| [T-230](T-230-infra.md) | Capture audio native (cpal, CoreAudio) sur un fil dédié | infra | P1 | [] |  |
| [T-231](T-231-tempo.md) | Analyse spectrale : 5 bandes, niveaux dBFS et gain automatique | tempo | P1 | [T-230] |  |
| [T-232](T-232-tempo.md) | Fonction d'onsets (flux spectral) et détection kick / caisse claire / charleston | tempo | P1 | [T-231] |  |
| [T-233](T-233-tempo.md) | Estimation du BPM et suivi des temps (autocorrélation, peigne, programmation dynamique) avec confiance | tempo | P1 | [T-232, T-150] |  |
| [T-234](T-234-tempo.md) | Brancher la détection sur l'horloge de tempo : verrouillage, maintien, tap prioritaire, recalage de phase | tempo | P1 | [T-233, T-150] |  |
| [T-237](T-237-live.md) | AudioFeatures v2 : instantané complet côté moteur et dans /api/state | live | P1 | [T-231] |  |
| [T-238](T-238-live.md) | Conditionnement des signaux audio : seuil, courbe, attaque/relâche, enveloppes en temps musicaux | live | P1 | [T-237, T-150] |  |
| [T-244](T-244-qa.md) | Banc d'essai de l'analyse audio : signaux synthétiques, corpus annoté, métriques, latence | qa | P1 | [T-231] |  |
| [T-255](T-255-safety.md) | Mode balayage public verrouillé par défaut | safety | P1 | [T-003, T-250, T-252, T-254, T-256, T-258] |  |
| [T-256](T-256-safety.md) | Garde anti-point fixe (taille minimum, vitesse, temps de pose) | safety | P1 | [T-250, T-003] |  |
| [T-258](T-258-safety.md) | Liste de contrôle avant show | safety | P1 | [T-250, T-259] |  |
| [T-259](T-259-safety.md) | Journal des événements de sécurité | safety | P1 | [] |  |
| [T-264](T-264-qa.md) | Suite de tests des invariants de sécurité | qa | P1 | [T-250, T-251, T-252, T-253, T-254, T-256] |  |
| [T-270](T-270-ui.md) | Nouvelle disposition de l'écran (régions, onglets de panneau) | ui | P1 | [] |  |
| [T-272](T-272-ui.md) | Grille de cues à taille fixe (8×5 par défaut), pages sur touches F | ui | P1 | [T-270] |  |
| [T-277](T-277-output.md) | Lieu et projecteurs multiples (modèle `Venue`, `/api/frames`) | output | P1 | [T-275] |  |
| [T-279](T-279-safety.md) | Surcouches de sécurité dans le visualiseur (zone public, horizon) | safety | P1 | [T-277, T-003] |  |
| [T-283](T-283-safety.md) | Mode spectacle (verrouillage) et protection contre les clics accidentels | safety | P1 | [T-270] |  |
| [T-286](T-286-infra.md) | Fichier projet `.lsproj` : ouvrir, enregistrer, récents | infra | P1 | [] |  |
| [T-287](T-287-infra.md) | Sauvegarde automatique et récupération après plantage | infra | P1 | [T-286] |  |
| [T-011](T-011-ilda.md) | Médiathèque ILDA : import et export depuis l'interface | ilda | P2 | [T-001] |  |
| [T-012](T-012-output.md) | Zones de projection et correction géométrique | output | P2 | [T-003] |  |
| [T-104](T-104-cues.md) | Croisements, faisceau chaud et convergences | cues | P2 | [T-102] |  |
| [T-107](T-107-cues.md) | Textures : ciel étoilé, éclairs, faisceaux épais | cues | P2 | [T-100] |  |
| [T-108](T-108-cues.md) | Graphismes : compte à rebours, anneau de progression, fil de fer 3D | cues | P2 | [T-100] |  |
| [T-112](T-112-cues.md) | Cue évolutif E1 « Éventail qui monte » (16 temps) | cues | P2 | [T-111, T-102, T-103, T-130] |  |
| [T-113](T-113-cues.md) | Cue évolutif E2 « Tunnel qui zoome sur le drop » (32 temps) | cues | P2 | [T-111, T-105, T-103] |  |
| [T-114](T-114-cues.md) | Cue évolutif E3 « Ciseaux croisés » (32 temps) | cues | P2 | [T-111, T-104] |  |
| [T-115](T-115-cues.md) | Cue évolutif E4 « Stabs sur 4 positions » (16 temps) | cues | P2 | [T-111, T-102, T-103] |  |
| [T-116](T-116-cues.md) | Cue évolutif E5 « Plafond qui descend » (32 temps) | cues | P2 | [T-111, T-106] |  |
| [T-117](T-117-cues.md) | Cue évolutif E6 « Soleil levant » (32 temps) | cues | P2 | [T-111, T-105, T-130] |  |
| [T-118](T-118-cues.md) | Cue évolutif E7 « Chaser qui accélère » (32 temps) | cues | P2 | [T-111, T-103] |  |
| [T-119](T-119-cues.md) | Cue évolutif E8 « Rayons tournants inversés » (32 temps) | cues | P2 | [T-111, T-105] |  |
| [T-120](T-120-cues.md) | Cue évolutif E9 « Vague » (16 temps) | cues | P2 | [T-111, T-102, T-130] |  |
| [T-121](T-121-cues.md) | Cue évolutif E10 « Couloir de rideaux » (32 temps) | cues | P2 | [T-111, T-106] |  |
| [T-122](T-122-cues.md) | Cue évolutif E11 « Étoiles puis éclatement » (16 temps) | cues | P2 | [T-111, T-107, T-104] |  |
| [T-123](T-123-cues.md) | Cue évolutif E12 « Grille qui se resserre » (32 temps) | cues | P2 | [T-111, T-106, T-101] |  |
| [T-124](T-124-timeline.md) | Timeline « Montée 16 mesures → drop » (32 mesures) | timeline | P2 | [T-160, T-111, T-112, T-113, T-114, T-118, T-119] |  |
| [T-125](T-125-timeline.md) | Timeline « Bloc techno » style Awakenings (64 mesures) | timeline | P2 | [T-160, T-111, T-123, T-115, T-104, T-106, T-105] |  |
| [T-126](T-126-timeline.md) | Timeline « Break trance → drop euphorique » (48 mesures) | timeline | P2 | [T-160, T-111, T-116, T-113, T-106, T-105] |  |
| [T-127](T-127-timeline.md) | Timeline « Attaque hardstyle » (150 BPM, 32 mesures) | timeline | P2 | [T-160, T-111, T-115, T-118, T-102] |  |
| [T-128](T-128-timeline.md) | Timeline « Entrée du DJ / tête d'affiche » (32 mesures) | timeline | P2 | [T-160, T-111, T-122, T-121, T-117, T-108] |  |
| [T-129](T-129-timeline.md) | Timeline « Hymne du coucher de soleil / clôture » (64 mesures) | timeline | P2 | [T-160, T-111, T-117, T-116, T-120, T-112, T-102] |  |
| [T-130](T-130-cues.md) | Palettes festival et équilibre perçu des couleurs | cues | P2 | [T-100] |  |
| [T-142](T-142-live.md) | Strobe, points visibles, pointillés, miroir/prisme, figer, noir momentané | live | P2 | [T-140, T-150] |  |
| [T-144](T-144-live.md) | Modificateurs au niveau cue et calque, sauvegarde dans le cue, lissage | live | P2 | [T-140, T-156] |  |
| [T-146](T-146-live.md) | Grille FX : effets par-dessus les cues | live | P2 | [T-140, T-151] |  |
| [T-153](T-153-live.md) | Routage des bandes audio vers n'importe quel contrôle | live | P2 | [T-145, T-151] |  |
| [T-158](T-158-cues.md) | Transitions entre cues : coupe, fondu, fondu au noir, morph | cues | P2 | [T-155, T-150] |  |
| [T-159](T-159-cues.md) | Lancement quantifié et mode beat (changement automatique au temps) | cues | P2 | [T-150, T-155] |  |
| [T-161](T-161-timeline.md) | Fichier audio et forme d'onde dans la timeline | timeline | P2 | [T-160] |  |
| [T-162](T-162-timeline.md) | Éditeur de timeline (pistes, glisser, magnétisme, zoom, marqueurs, copier-coller) | ui | P2 | [T-160, T-161] |  |
| [T-163](T-163-timeline.md) | Enveloppes de paramètres sur n'importe quel contrôle | timeline | P2 | [T-160, T-145] |  |
| [T-164](T-164-cues.md) | Éditeur de cue évolutif | ui | P2 | [T-157, T-162] |  |
| [T-165](T-165-timeline.md) | Modèles de timeline (phrases prêtes à poser) | timeline | P2 | [T-160, T-163] |  |
| [T-171](T-171-output.md) | Budget de points, vitesse de balayage et minimum de points par sortie | output | P2 | [T-002] |  |
| [T-206](T-206-midi.md) | Couleurs RGB des pads sur l'APC40 mkII (couleur de la cue, pulsation) | midi | P2 | [T-205] |  |
| [T-210](T-210-ui.md) | APC40 virtuel à l'écran (disposition, affectations, état des LED) | ui | P2 | [T-203, T-204, T-205] |  |
| [T-235](T-235-tempo.md) | Détection du temps fort (début de mesure) et des phrases de 8/16 mesures | tempo | P2 | [T-234, T-236] |  |
| [T-236](T-236-tempo.md) | Détection montée / drop / break et silence (sections musicales) | tempo | P2 | [T-231, T-232] |  |
| [T-239](T-239-live.md) | Préréglages de réactivité audio (correspondances bandes → paramètres laser) | live | P2 | [T-153, T-238] |  |
| [T-240](T-240-cues.md) | Déclencheurs sur événements audio (kick, drop, break → cues) | cues | P2 | [T-232, T-236, T-155] |  |
| [T-242](T-242-ui.md) | Permission micro macOS et diagnostic de l'entrée audio | ui | P2 | [T-230] |  |
| [T-243](T-243-ui.md) | Panneau « Musique » v2 : bandes, spectre, onsets, tempo détecté, section | ui | P2 | [T-237, T-233, T-236] |  |
| [T-246](T-246-tempo.md) | Compensation de latence : décalage de sortie réglable et temps prédits | tempo | P2 | [T-234] |  |
| [T-257](T-257-safety.md) | Estimateur d'exposition (EMP) et distance de danger (DNRO) | safety | P2 | [T-254] |  |
| [T-260](T-260-safety.md) | Profils de sécurité par lieu | safety | P2 | [T-003, T-254, T-101] |  |
| [T-262](T-262-safety.md) | Mode extérieur — angles déclarés, « Ciel coupé », limite 45° | safety | P2 | [T-003, T-250, T-254, T-259] |  |
| [T-273](T-273-ui.md) | Vignettes animées des cues | ui | P2 | [T-272] |  |
| [T-274](T-274-cues.md) | Aperçu avant diffusion (préparer un cue sans l'envoyer) | cues | P2 | [T-270] |  |
| [T-278](T-278-ui.md) | Points de vue caméra (public, premier rang, scène, dessus, côté) | ui | P2 | [T-277] |  |
| [T-281](T-281-ui.md) | Fenêtres supplémentaires et multi-écran (`?vue=`) | ui | P2 | [T-270, T-275] |  |
| [T-282](T-282-ui.md) | Mode nuit et mode tactile | ui | P2 | [T-270] |  |
| [T-284](T-284-ui.md) | Table unique des raccourcis clavier et aide « ? » | ui | P2 | [T-270] |  |
| [T-285](T-285-ui.md) | Annuler / rétablir et historique des modifications | ui | P2 | [T-286] |  |
| [T-288](T-288-infra.md) | Versions du format (migrations) et versions nommées | infra | P2 | [T-286] |  |
| [T-289](T-289-infra.md) | Import partiel, profil de site et paquet d'export `.lspack` | infra | P2 | [T-286, T-288] |  |
| [T-013](T-013-midi.md) | Entrées OSC et Art-Net/DMX | midi | P3 | [] |  |
| [T-109](T-109-output.md) | Cibles miroir : faisceaux dirigés vers des points calibrés | output | P3 | [T-003, T-100] |  |
| [T-148](T-148-cues.md) | Pilote automatique (Virtual LJ) calé sur le tempo | cues | P3 | [T-159] |  |
| [T-154](T-154-tempo.md) | Ableton Link : étude de licence et intégration optionnelle | tempo | P3 | [T-150] |  |
| [T-166](T-166-timeline.md) | Pack de modèles intégrés (nos propres phrases) | timeline | P3 | [T-165] |  |
| [T-167](T-167-timeline.md) | Génération automatique d'un show depuis l'analyse du morceau | timeline | P3 | [T-165, T-161, T-152] |  |
| [T-168](T-168-timeline.md) | Timecode entrant (MTC, puis LTC par l'entrée audio) | timeline | P3 | [T-160] |  |
| [T-169](T-169-timeline.md) | Enregistrer le jeu en direct dans la timeline | timeline | P3 | [T-160, T-145] |  |
| [T-170](T-170-output.md) | Groupes de projecteurs et chenillard entre zones | output | P3 | [T-012, T-150] |  |
| [T-207](T-207-midi.md) | Horloge MIDI (entrée pour caler le BPM, sortie vers l'APC40 mkII) | tempo | P3 | [T-200, T-150] |  |
| [T-241](T-241-infra.md) | Capturer le son du Mac lui-même (BlackHole documenté, puis capture système optionnelle) | infra | P3 | [T-230] |  |
| [T-261](T-261-safety.md) | Verrouillage des réglages de sécurité par code | safety | P3 | [T-260, T-259] |  |
| [T-263](T-263-safety.md) | Fiche sécurité du show exportable | safety | P3 | [T-254, T-258, T-262] |  |
| [T-280](T-280-ui.md) | Simulation de l'inertie des galvos et divergence | ui | P3 | [T-276, T-171] |  |

## Bloqué (2)

| id | tâche | domaine | priorité | dépend de | branche |
|---|---|---|---|---|---|
| [T-015](T-015-output.md) | Sortie ShowNET (API Laserworld) | output | P0 | [] |  |
| [T-152](T-152-tempo.md) | Détection automatique du BPM depuis l'audio (avec confiance) | tempo | P2 | [T-150] |  |

## Fait (30)

| id | tâche | domaine | priorité | dépend de | branche |
|---|---|---|---|---|---|
| [T-208](T-208-safety.md) | Sécurité du pilotage MIDI (blackout prioritaire, armement opt-in, reprise en douceur) | safety | P0 | [T-202] | feat/midi-map |
| [T-250](T-250-safety.md) | Verrous d'armement (interlocks) et raisons de désarmement | safety | P0 | [] | feat/arming |
| [T-251](T-251-safety.md) | Arrêt d'urgence verrouillé (clavier, bouton, API, MIDI) | safety | P0 | [T-250] | feat/arming |
| [T-252](T-252-safety.md) | Présence opérateur — battement de l'interface et mode maintien | safety | P0 | [T-250] | feat/heartbeat |
| [T-004](T-004-qa.md) | Tests e2e qui cliquent (Playwright) | qa | P1 | [] | feat/e2e |
| [T-005](T-005-cues.md) | Bibliothèque de 202 cues procéduraux | cues | P1 | [] | feat/presets |
| [T-100](T-100-cues.md) | Générateurs cadencés au beat (beat_pos, bpm, groupes) | cues | P1 | [T-150] | feat/beat-gen |
| [T-101](T-101-safety.md) | Limiteur de stroboscope et horizon appliqués à tous les looks | safety | P1 | [T-100] | feat/strobe-limit |
| [T-102](T-102-cues.md) | Générateurs éventails : fan, balayage, levée, ouverture, vague, positions | cues | P1 | [T-100] | feat/fan-gens |
| [T-111](T-111-cues.md) | Moteur de cues évolutifs (images clés en temps) | cues | P1 | [T-100] | feat/evolving |
| [T-140](T-140-live.md) | Étage de modificateurs en direct maître (géométrie, luminosité, vitesse) | live | P1 | [T-145] | feat/live |
| [T-141](T-141-live.md) | Couleur en direct : fixe, teinte, palette, arc-en-ciel, chenillard | live | P1 | [T-140, T-150] | feat/live-color |
| [T-145](T-145-live.md) | Identifiants de contrôle stables (registre des contrôles) | live | P1 | [] | feat/controls |
| [T-150](T-150-tempo.md) | Moteur de tempo : BPM, tap, resync, phase temps/mesure | tempo | P1 | [] | feat/tempo |
| [T-151](T-151-tempo.md) | Modulateurs LFO synchronisés au tempo sur n'importe quel contrôle | tempo | P1 | [T-150, T-145] | feat/lfo |
| [T-155](T-155-cues.md) | Modes de déclenchement des cues, groupes exclusifs, limiteur | cues | P1 | [T-145] | feat/cue-modes |
| [T-156](T-156-cues.md) | Quatre calques avec gradateur, muet/solo et budget de points | cues | P1 | [T-155, T-140] | feat/layers |
| [T-200](T-200-midi.md) | Entrée/sortie MIDI native (midir, CoreMIDI) | midi | P1 | [] | feat/midi-core |
| [T-201](T-201-midi.md) | Détection des contrôleurs et profils par appareil (APC40 / APC40 mkII) | midi | P1 | [T-200] | feat/midi-core |
| [T-202](T-202-midi.md) | Moteur de correspondances MIDI → contrôles (boutons, faders, encodeurs, Shift) | midi | P1 | [T-200, T-201, T-145] | feat/midi-map |
| [T-209](T-209-qa.md) | Tests MIDI sans matériel (APC40 simulé, ports virtuels, injection e2e) | qa | P1 | [T-200] | feat/midi-tests |
| [T-253](T-253-safety.md) | Chien de garde du moteur et extinction propre | safety | P1 | [T-250] | feat/heartbeat |
| [T-275](T-275-ui.md) | Visualiseur 3D : socle WebGL2, salle, caméra orbitale, un projecteur | ui | P1 | [] | feat/beam-view |
| [T-276](T-276-ui.md) | Rendu des faisceaux : énergie conservée, nappes, brume, halo | ui | P1 | [T-275] | feat/beam-view |
| [T-291](T-291-ui.md) | Raccourcis clavier morts après un curseur ou une case à cocher (Espace n'éteint plus) | ui | P1 | [] | fix/arm-keys |
| [T-293](T-293-ui.md) | Pendant la playlist, un curseur du look renvoie l'ancien look (la scène saute) | ui | P1 | [] | fix/ui-state |
| [T-160](T-160-timeline.md) | Timeline : modèle de show et lecteur (pistes, événements, carte de tempo) | timeline | P2 | [T-150, T-156] | feat/timeline |
| [T-290](T-290-safety.md) | Bouton laser / Espace basculent depuis une copie locale périmée de « armed » | safety | P2 | [] | fix/arm-keys |
| [T-292](T-292-cues.md) | La cue active reste « en cours » côté serveur après un changement de look à la main | cues | P2 | [] | fix/ui-state |
| [T-294](T-294-qa.md) | Test e2e intermittent : « Synchro tempo keeps the preset step » (live.spec.ts:43) | qa | P3 | [] |  |
