# Index des tâches

_Généré par `python3 tasks/make_index.py` — ne pas éditer à la main._

## À faire (80)

| id | tâche | domaine | priorité | dépend de | branche |
|---|---|---|---|---|---|
| [T-003](T-003-safety.md) | Zones de sécurité, horizon, calibration couleur | safety | P0 | [] |  |
| [T-208](T-208-safety.md) | Sécurité du pilotage MIDI (blackout prioritaire, armement opt-in, reprise en douceur) | safety | P0 | [T-202] |  |
| [T-001](T-001-ilda.md) | Lecteur/écrivain ILDA maison | ilda | P1 | [] |  |
| [T-002](T-002-output.md) | Optimiseur de points (tracé laser pro) | output | P1 | [] |  |
| [T-004](T-004-qa.md) | Tests e2e qui cliquent (Playwright) | qa | P1 | [] |  |
| [T-100](T-100-cues.md) | Générateurs cadencés au beat (beat_pos, bpm, groupes) | cues | P1 | [T-150] |  |
| [T-101](T-101-safety.md) | Limiteur de stroboscope et horizon appliqués à tous les looks | safety | P1 | [T-100] |  |
| [T-102](T-102-cues.md) | Générateurs éventails : fan, balayage, levée, ouverture, vague, positions | cues | P1 | [T-100] |  |
| [T-103](T-103-cues.md) | Chasers, coups sur le kick et strobes de faisceaux | cues | P1 | [T-100, T-101] |  |
| [T-105](T-105-cues.md) | Tunnels, cônes, soleil et rayons tournants | cues | P1 | [T-100] |  |
| [T-106](T-106-cues.md) | Nappes : liquid sky, lame, rideaux, cascade, scanner, lamelles, aurore, grille | cues | P1 | [T-100] |  |
| [T-110](T-110-cues.md) | Page de cues « Festival » | cues | P1 | [T-102, T-103, T-104, T-105, T-106, T-107, T-130] |  |
| [T-111](T-111-cues.md) | Moteur de cues évolutifs (images clés en temps) | cues | P1 | [T-100] |  |
| [T-140](T-140-live.md) | Étage de modificateurs en direct maître (géométrie, luminosité, vitesse) | live | P1 | [T-145] |  |
| [T-141](T-141-live.md) | Couleur en direct : fixe, teinte, palette, arc-en-ciel, chenillard | live | P1 | [T-140, T-150] |  |
| [T-143](T-143-ui.md) | Panneau « Direct » (modificateurs en direct dans l'interface) | ui | P1 | [T-140, T-141, T-150] |  |
| [T-145](T-145-live.md) | Identifiants de contrôle stables (registre des contrôles) | live | P1 | [] |  |
| [T-150](T-150-tempo.md) | Moteur de tempo : BPM, tap, resync, phase temps/mesure | tempo | P1 | [] |  |
| [T-151](T-151-tempo.md) | Modulateurs LFO synchronisés au tempo sur n'importe quel contrôle | tempo | P1 | [T-150, T-145] |  |
| [T-155](T-155-cues.md) | Modes de déclenchement des cues, groupes exclusifs, limiteur | cues | P1 | [T-145] |  |
| [T-156](T-156-cues.md) | Quatre calques avec gradateur, muet/solo et budget de points | cues | P1 | [T-155, T-140] |  |
| [T-157](T-157-cues.md) | Cues évolutifs : un cue = une mini-timeline (calques internes, courbes sur modificateurs, LFO) | cues | P1 | [T-111, T-151, T-140] |  |
| [T-200](T-200-midi.md) | Entrée/sortie MIDI native (midir, CoreMIDI) | midi | P1 | [] |  |
| [T-201](T-201-midi.md) | Détection des contrôleurs et profils par appareil (APC40 / APC40 mkII) | midi | P1 | [T-200] |  |
| [T-202](T-202-midi.md) | Moteur de correspondances MIDI → contrôles (boutons, faders, encodeurs, Shift) | midi | P1 | [T-200, T-201, T-145] |  |
| [T-203](T-203-midi.md) | Apprentissage MIDI (clic droit → « Apprendre MIDI ») | midi | P1 | [T-202, T-145, T-209] |  |
| [T-204](T-204-midi.md) | Profil APC40 par défaut (disposition Laser Studio) pour APC40 et APC40 mkII | midi | P1 | [T-201, T-202, T-140, T-145, T-150, T-155, T-160, T-208, T-209] |  |
| [T-205](T-205-midi.md) | Retour LED sur l'APC40 (cue active, page, calques, battement) | midi | P1 | [T-201, T-204, T-150] |  |
| [T-209](T-209-qa.md) | Tests MIDI sans matériel (APC40 simulé, ports virtuels, injection e2e) | qa | P1 | [T-200] |  |
| [T-211](T-211-midi.md) | Contrôleurs MIDI génériques (n'importe quel appareil) | midi | P1 | [T-200, T-201, T-202, T-203] |  |
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
| [T-152](T-152-tempo.md) | Détection automatique du BPM depuis l'audio (avec confiance) | tempo | P2 | [T-150] |  |
| [T-153](T-153-live.md) | Routage des bandes audio vers n'importe quel contrôle | live | P2 | [T-145, T-151] |  |
| [T-158](T-158-cues.md) | Transitions entre cues : coupe, fondu, fondu au noir, morph | cues | P2 | [T-155, T-150] |  |
| [T-159](T-159-cues.md) | Lancement quantifié et mode beat (changement automatique au temps) | cues | P2 | [T-150, T-155] |  |
| [T-160](T-160-timeline.md) | Timeline : modèle de show et lecteur (pistes, événements, carte de tempo) | timeline | P2 | [T-150, T-156] |  |
| [T-161](T-161-timeline.md) | Fichier audio et forme d'onde dans la timeline | timeline | P2 | [T-160] |  |
| [T-162](T-162-timeline.md) | Éditeur de timeline (pistes, glisser, magnétisme, zoom, marqueurs, copier-coller) | ui | P2 | [T-160, T-161] |  |
| [T-163](T-163-timeline.md) | Enveloppes de paramètres sur n'importe quel contrôle | timeline | P2 | [T-160, T-145] |  |
| [T-164](T-164-cues.md) | Éditeur de cue évolutif | ui | P2 | [T-157, T-162] |  |
| [T-165](T-165-timeline.md) | Modèles de timeline (phrases prêtes à poser) | timeline | P2 | [T-160, T-163] |  |
| [T-171](T-171-output.md) | Budget de points, vitesse de balayage et minimum de points par sortie | output | P2 | [T-002] |  |
| [T-206](T-206-midi.md) | Couleurs RGB des pads sur l'APC40 mkII (couleur de la cue, pulsation) | midi | P2 | [T-205] |  |
| [T-210](T-210-ui.md) | APC40 virtuel à l'écran (disposition, affectations, état des LED) | ui | P2 | [T-203, T-204, T-205] |  |
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

## Bloqué (1)

| id | tâche | domaine | priorité | dépend de | branche |
|---|---|---|---|---|---|
| [T-015](T-015-output.md) | Sortie ShowNET (API Laserworld) | output | P0 | [] |  |

## Fait (1)

| id | tâche | domaine | priorité | dépend de | branche |
|---|---|---|---|---|---|
| [T-005](T-005-cues.md) | Bibliothèque de 202 cues procéduraux | cues | P1 | [] | feat/presets |
