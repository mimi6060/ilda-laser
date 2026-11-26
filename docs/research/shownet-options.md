# Piloter le ShowNET depuis Laser Studio : les options

Question : comment faire sortir Laser Studio (macOS) sur le **Laserworld
ShowNET** de l'utilisateur (interface externe, firmware 2016050502, DIP tous
OFF = mode réseau, licences Showeditor / Showcontroller / Dummy1–4), alors que
le protocole de streaming est chiffré et que Laserworld ne répond plus à la
demande d'API sous NDA ?

Méthode : informations **publiques uniquement** (sites, manuels PDF, FAQ,
forums, GitHub, communiqués). Rien n'a été décompilé, désassemblé, sniffé ni
téléchargé en binaire ; aucune clé n'a été cherchée. Toutes les sources ont été
consultées le **2026-10-03**. Complète `docs/research/showcontroller.md` §4
(ShowNET) et `docs/research/madmapper-and-open-content.md`.

---

## Résumé (TL;DR)

1. **Aucun SDK public.** Laserworld ne donne sa bibliothèque ShowNET
   (Windows, Mac Intel/M1, Linux expérimental) qu'aux **partenaires
   commerciaux / projets publics**, sous un « petit NDA », avec des clés par
   partenaire. Les logiciels compatibles (MadMapper, TouchDesigner, ILD
   Render, Millumin, Modulaser, CloudLase…) passent tous par ce programme.
2. **La voie légale et immédiate : PONK → MadMapper → ShowNET.** MadMapper
   (MadLaser) reçoit depuis la version 5.2 les tracés laser d'autres
   logiciels via **PONK**, un protocole UDP **ouvert (Apache-2.0)** publié par
   MadMapper, et les sort sur le ShowNET avec sa licence officielle. Il existe
   déjà une crate Rust MIT (`ponk-protocol`). Une petite sortie `PonkOutput`
   suffit : quelques jours de travail, sans toucher au ShowNET.
3. **La voie autonome : un DAC ouvert à la place du ShowNET.** Le ShowNET
   n'est qu'un boîtier réseau → ILDA DB25. Un **HeliosPRO** (≈ 185 €, IDN)
   marche **déjà** avec notre feature `idn` de `laser-dac`, sans code. Un
   **Helios USB** (≈ 99 €) demande juste d'activer la feature `helios`. Un
   **Ether Dream 4** (≈ 229–289 $) marche déjà.
4. **Le ShowNET sait aussi jouer seul sa carte SD, déclenché en Art-Net** : il joue les fichiers `001.ild`–`229.ild` de sa carte microSD,
   choisis par un canal DMX. Il faut mettre à jour le firmware (le mode
   Art-Net est documenté à partir du firmware 2019, l'outil de mise à jour est
   Windows, à lancer une fois), passer en DHCP et changer les DIP. Notre studio pourrait alors
   exporter la banque SD et piloter le boîtier en Art-Net. C'est limité (pas
   de streaming), mais 100 % documenté.
5. **Continuer à relancer Laserworld** pour le SDK (le dépôt GitHub public du
   projet coche la case « projet publiquement disponible »), par plusieurs
   canaux (liste au §4).

**Plan recommandé** (§8) : faire tout de suite T-300 (sortie PONK), acheter un
HeliosPRO comme sortie de secours sans dépendance (T-303 pour l'UI), et
garder l'Art-Net SD (T-301, T-302) comme mode autonome. Le `ShowNetOutput`
natif reste en attente du SDK.

---

## 1. SDK / API Laserworld et logiciels compatibles

### 1.1 Le programme API/SDK

| Fait | Source |
|---|---|
| Bibliothèque « for Windows as well as for Mac systems, and even a version for Linux-based systems (experimental) » | [Page API/SDK Laserworld](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html) |
| « The API data is only provided to commercial integration partners. If you have a publicly available software project, this is exactly for you. » Les projets industriels avec logiciel dédié sont aussi acceptés ; **les projets privés non**. | idem |
| « we make a little NDA with each integration partner » ; clés individuelles par partenaire. Il faut envoyer les détails du projet et **l'adresse légale** pour le NDA. Contact : « Norbert » (e-mail masqué en JavaScript). Aucun prix publié. | idem |
| Annonce du 27/01/2022 : Windows x86/x64, macOS Intel et M1 ; inscription via `laser-interface.com/en/sdk` ; **« even older hardware dating back to 2015 can be controlled after applying the latest firmware update »** | [PLSN, 2022](https://plsn.com/newsroom/product-news/laserworld-announces-availability-of-shownet-laser-api/), [ETNow](https://www.etnow.com/news/2022/1/shownet-laser-api-available) |
| L'adresse `laser-interface.com/en/sdk` redirige aujourd'hui vers une URL cassée de laserworld.com (constaté le 2026-10-03). | test direct |

Conséquences pour nous :
- Notre dépôt est **public sur GitHub** (`mimi6060/ilda-laser`, il ressort
  dans les recherches web sur le ShowNET). C'est l'argument à mettre en avant :
  « publicly available software project ».
- Le NDA porte sur les **clés**. Le code de la future `ShowNetOutput` devra
  lire la clé et la bibliothèque **hors dépôt** (variable d'environnement ou
  fichier dans `studio-data/`), jamais commités.
- Firmware : le communiqué suppose le **dernier firmware**. Le nôtre date de
  2016. À demander à Laserworld avant de flasher (voir §2.4).

### 1.2 Qui sort sur le ShowNET, et comment

Liste officielle « directly compatible »
([laserworld.com/en/shownet-compatible](https://www.laserworld.com/en/shownet-compatible.html)) :
Showcontroller, Showeditor, MadMapper/MadLaser, TouchDesigner, ILD Render,
Millumin, Modulaser, CloudLase Studio.

| Logiciel | Mac ? | ShowNET | Entrées utiles pour nous | Source |
|---|---|---|---|---|
| **MadMapper + MadLaser** | oui (macOS 11+) | natif, « no additional software or licenses » côté ShowNET | **PONK in/out (5.2+)**, OSC, MIDI, DMX/Art-Net, import ILDA. MadLaser : essai gratuit (sortie laser coupée toutes les 120 s), location dès 19 €/mois, perpétuel dès 199 € HT | [MadLaser](https://madmapper.com/madlaser/extension), [notes 5.2](https://forum.garagecube.com/viewtopic.php?t=35826), [FAQ extensions](https://madmapper.com/extensions/faq), [PLSN](https://plsn.com/featured/madmapper-releases-madlaser-with-native-shownet-implementation/) |
| **TouchDesigner** | oui (app) | « ShowNET hardware is interfaced to TouchDesigner on a Ethernet port » ; la doc ne dit pas si c'est dispo sur Mac | PONK (composant gratuit financé par MadMapper), OSC… | [Derivative – Lasers](https://docs.derivative.ca/Lasers), [PONK pour TD](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438) |
| **Modulaser** | oui (macOS + Windows, pas Linux pour ShowNET) | DAC ShowNET, projecteur en DHCP ; « Only one application can control the projector at a time » | MIDI, OSC, audio | [Modulaser – ShowNET](https://modulaser.app/docs/outputs/laserworld-shownet) |
| **CloudLase Studio** | oui (macOS, Windows, Linux) | « ShowNET (Windows and macOS) » ; formule gratuite, Pro 59 $ à vie | aucune entrée de flux externe documentée | [cloudlase.studio](https://cloudlase.studio/) |
| **ILD Render** (Blender/Inkscape) | — | sortie directe jusqu'à 9 ShowNET ; logiciel commercial | export ILDA | [Laserworld news](https://www.laserworld.com/en/newslist/5630-ild-render-supports-direct-output-to-the-shownet-laser-mainboard.html), [PLSN](https://plsn.com/newsroom/product-news/ild-render-supports-direct-output-to-shownet-laser-mainboard/) |
| Showcontroller / Showeditor | **non** (Windows) | natif | Showcontroller LIVE : télécommande Art-Net/MIDI (tableau 11 canaux) | `showcontroller.md` §1, §4 |

Tous ont un accès **sous licence Laserworld** (partenariat SDK). Aucun ne
publie la bibliothèque. Aucun ne revend un « pont » générique, **sauf** que
MadMapper et TouchDesigner acceptent des tracés venus d'autres logiciels via
**PONK** : c'est ce qui nous intéresse.

### 1.3 PONK, le pont légal

| Fait | Source |
|---|---|
| « PONK (Pathes Over NetworK) … minimal protocol to transfer 2D colored pathes from a source to a receiver », UDP multicast (par défaut) ou unicast ; licence **Apache-2.0** ; dépôt `madmappersoftware/Ponk` | [GitHub Ponk](https://github.com/madmappersoftware/Ponk) |
| En-tête `PONK-UDP`, version 0, id et nom d'émetteur, n° de frame, découpage en morceaux ≤ 8192 octets, CRC ; chemins en `XY_F32_RGB_U8` ; métadonnées par chemin (clé 8 caractères, valeur float) dont des options MadLaser (`MAXSPEED`, `SKIPBLCK`, `ANGLEOPT`…) | idem |
| Groupe multicast conventionnel 239.255.10.24, port 5583. Un récepteur abonné au multicast reçoit **aussi** l'unicast sur ce port. Le spec ne fixe pas l'échelle des coordonnées (à vérifier dans MadMapper) | [README ponk-protocol](https://github.com/ModulaserApp/ponk-protocol), [spec Ponk](https://github.com/madmappersoftware/Ponk) |
| MadMapper re-rend les chemins (ordre, vitesse, angles, points mini) ; des métadonnées par chemin (`PRESRVOR`, `ANGLEOPT`, `MINIPNTS`, `MAXSPEED`…) surchargent ses réglages | [spec Ponk](https://github.com/madmappersoftware/Ponk) |
| « MadLaser: added PONK support (in & out) » — MadMapper 5.2 (nov. 2022) | [forum garageCube](https://forum.garagecube.com/viewtopic.php?t=35826), [CDM](https://cdm.link/2022/11/madmapper-5-2-more-lasers-kinect/) |
| Crate Rust **`ponk-protocol` 0.1.0, MIT, zéro dépendance**, encode/décode, réassemblage borné ; « wire behavior validated against canonical MadMapper/GarageCube PONK implementation » ; MSRV 1.88 (on a 1.98) ; rappelle qu'elle n'implémente **ni armement, ni blanking, ni limites de vitesse** | [docs.rs](https://docs.rs/ponk-protocol/latest/ponk_protocol/), [GitHub ModulaserApp/ponk-protocol](https://github.com/ModulaserApp/ponk-protocol) |
| **Piège sécurité** : si l'émetteur n'envoie rien pour une frame vide, « MadMapper gets nothing and keeps displaying the last frame it received ». Il faut **toujours** envoyer au moins un paquet par frame, même vide. | [ofxPonk issue #2 (01/10/2026)](https://github.com/jonasfehr/ofxPonk/issues/2) |

Ce que ça veut dire : **Laser Studio → PONK (localhost) → MadMapper/MadLaser
→ ShowNET**. Aucun secret Laserworld n'est touché ; MadMapper utilise sa
licence officielle ; le protocole est ouvert. Coût : une licence MadLaser
(si l'utilisateur n'en a pas déjà une) et MadMapper qui tourne à côté.

---

## 2. Protocoles ouverts du ShowNET lui-même

### 2.1 Ce que le boîtier sait faire sans PC

Source principale : [manuel ShowNET 05/2019, firmware 20190520x, Admin Tool 1.33](https://audioeffetti.com/product/documents/LAS/LASERWORLD%20SHOWN-01.pdf)
(même document que [laserworld.com, manuel 2019](https://www.laserworld.com/en/download-file-1242-Laserworld_ShowNET_Manual_2019_A6_web.html)).

- **Stand-alone / auto** : joue en boucle les `.ild` de la microSD. DIP 4 =
  une seule figure en boucle ; DIP 1/2 = suivante/précédente.
- **Démo** : idem + animations automatiques internes.
- **DMX / Art-Net** : DIP 10 ON, DIP 1–9 = adresse de départ en binaire
  (1, 2, 4 … 256). Le ShowNET externe standard **n'a pas de prise DMX** : il
  faut l'adaptateur DMX Laserworld
  ([FAQ](https://www.laserworld.com/en/shownet-faq/5894-how-can-i-use-dmx-to-trigger-the-external-shownet-interface.html)),
  **ou l'Art-Net** par le câble réseau.
- **Art-Net** : « ArtNet operation requires the ShowNET interface to be
  connected in a DHCP environment » ; dans l'Admin Tool, onglet *Settings*,
  « Data source for internal DMX effects » = *ArtNet input* ; « Only ShowNET
  interfaces and the ArtNet controller must be used in the same network ».
- **Maître/esclave** et **son** (mainboard avec micro).
- **IDN, OSC, timecode** : rien trouvé dans les manuels, la FAQ ni les
  pages produit. Le ShowNET PRO ajoute DMX in/out et un convertisseur Art-Net
  intégré, pas d'IDN
  ([ShowNET PRO](https://www.laserworld.com/en/laser-software/laserworld-shownet-pro-interface.html)).
- **Aucun produit Laserworld ne parle IDN** d'après toutes les pages
  publiques consultées (recherche « Laserworld IDN » sans résultat pertinent).

Le **manuel 2015** du même boîtier ne parle que de DMX filaire et de figures
internes, **pas d'Art-Net**
([manuel 2015-11](https://www.laserworld.com/en/download-file-1241-0_Laserworld_Manual_ShowNET_2015_11.html)).
Notre firmware 2016050502 est entre les deux : **l'Art-Net n'est pas
garanti** sans mise à jour.

### 2.2 Carte des canaux (profil DJ, 19 canaux, manuel 2019)

| Can. | Fonction | Valeurs clés |
|---|---|---|
| 1 | Intensité | 0 = laser éteint ; 1–255 (recommandé 64–192) |
| 2 | Figure | 0 = blackout (`000.ild` ne doit pas exister) ; n = `nnn.ild` ; numéro vide = blackout |
| 3 | Vitesse (fps) | 0–15 = 50 fps ; 16–255 = 0 → 100 fps |
| 4 | Taille | 0–127 X+Y, 128–191 X, 192–255 Y |
| 5 | Taille auto | |
| 6 | Rotation | 0–192 manuelle, 193–224 / 225–255 auto |
| 7–10 | Position X/Y grossier/fin | |
| 11–12 | Couleurs / fondus | 0–15 = couleurs d'origine |
| 13 | Strobe | 0–15 aucun |
| 14 | Mode | 0–19 DMX ; 20–211 positions auto ; 212–233 démo ; 234–255 son |
| 15 | Vitesse de scan | 0–31 défaut ; jusqu'à 30 kpps (« stay with the default ») |
| 16–17 | Zone de sécurité (taille/côté, intensité) | |
| 18–19 | Blanking / décalage | |

Profil **Professionnel** (34 canaux, 16 bits, RVB direct, réglages
géométriques) : [tableau PDF](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_professional_Mode.pdf)
et [Open Fixture Library](https://open-fixture-library.org/laserworld/shownet)
(données du dépôt OFL, licence MIT). Tableau DJ :
[PDF](https://www.laserworld.com/images/al_gfx/online-manual/DMX-tables/DMX-Table-ShowNET-Laser-Mainboard_-_DJ_Mode.pdf).
Un tableau de canaux est un fait technique ; on le réécrit nous-mêmes dans
notre code, sans copier de fichier.

### 2.3 Comment Laser Studio pourrait s'en servir

Laser Studio enverrait des paquets **ArtDmx** (UDP 6454, protocole Art-Net
public) au ShowNET. Chaque cue du studio correspondrait à un numéro de figure
de la carte SD. Le mapping minimal :

- désarmé / Échap / e-stop → canal 1 = 0 **et** canal 2 = 0, envoyés tout de
  suite et répétés (pas seulement « arrêter d'envoyer » : on ne sait pas ce
  que le firmware fait si l'Art-Net s'arrête, à vérifier sans laser) ;
- armé → canal 1 = master × luminosité, canal 2 = n° de figure du cue actif,
  canal 14 = 0 (mode DMX), canal 3 = vitesse, etc.

Limites honnêtes :
- **Pas de streaming** : seulement des figures pré-enregistrées sur la carte,
  plus les effets internes du ShowNET (taille, rotation, couleurs, position).
  Nos générateurs, nos LFO et notre audio-réactivité ne passent qu'à travers
  ces quelques canaux.
- **Nos protections** (limitation, calibration, zones, blanking entre formes)
  ne s'appliquent qu'au moment de l'**export** des `.ild`, pas en direct. En
  direct on ne contrôle que l'intensité et les zones du ShowNET (can. 16–17).
- Le mode Art-Net et le mode streaming (MadMapper) s'excluent : il faut
  changer les DIP et redémarrer le boîtier pour passer de l'un à l'autre
  (« Do not change … DIP switch settings during operation, random and
  dangerous laser output can occur »).

### 2.4 Pré-requis côté utilisateur (actions manuelles, hors code)

1. Demander à Laserworld si une mise à jour depuis 2016050502 **conserve les
   licences** (Showeditor/Showcontroller/Dummy) stockées dans le boîtier.
   Rien de public là-dessus.
2. Firmware actuel : **2025120902** ; l'historique liste 2025043002 (« changes
   the behavior of the scaling in DMX / Art-Net mode »), 2025011502,
   2022090202, 2021010602 ; 2015080802 est marqué « Outdated … Do not use for
   production »
   ([téléchargements actuels](https://www.laserworld.com/en/download-category-software-current-shownet.html),
   [anciens](https://www.laserworld.com/en/download-category-software-old-shownet.html),
   [2025043002](https://www.laserworld.com/de/download-file-2018-application_shownet_2025043002.html)).
3. Outil : **ShowNET Admin Tool 1.39, Windows (.exe)**. Il faut un PC Windows
   (ou une VM). Après le flash : couper le secteur et rallumer
   ([page firmware](https://www.laserworld.com/en/download-file-1429-application_shownet_2021010602.html)).
   L'Admin Tool ne se connecte pas pendant que Showeditor/Showcontroller
   utilise le boîtier ([vidéo](https://www.laserworld.com/en/video-detail-Sqw4h0_uEiQ.html)).
4. Pour l'Art-Net : réseau avec serveur DHCP (box/routeur), DIP 10 ON +
   adresse DMX sur DIP 1–9, profil DJ, « Data source … = ArtNet input ».

Ces étapes utilisent les outils officiels de Laserworld comme prévu : c'est
l'utilisateur qui les fait, pas un agent.

---

## 3. Carte SD : exporter notre contenu

- Format exigé : **ILDA format 5 (RVB vrai)** ; noms **`000.ild`–`255.ild`**
  uniquement ; le numéro = valeur DMX du canal figure ; `000` à laisser vide ;
  **230–255 réservés** (faisceaux, mires) ; ≤ 8 Mo par fichier (Admin Tool
  ≤ 6 Mo, au-delà lecteur de carte)
  ([guide contenu perso](https://www.laserworld.com/en/laser-online-user-manual/how-to-create-and-upload-custom-laser-content.html),
  [FAQ ILDA non reconnus](https://www.laserworld.com/en/laserworld-showeditor-faq/2572-why-are-my-ilda-ild-files-are-not-recognized-by-the-shownet-interface.html),
  détails dans `showcontroller.md` §4.2).
- Le manuel 2019 dit qu'on peut copier les `.ild` « with a standard card
  reader » : un Mac avec lecteur microSD suffit, **pas besoin de Windows**
  pour la carte elle-même.
- **Légal** : exporter **nos propres** looks (générés par le studio) en ILDA
  est sans problème. C'est déjà prévu par T-001 (écrivain format 5 +
  `shownet_sd_name`) et T-011 (export depuis l'UI). Rien n'oblige à passer
  par Showeditor. Interdits (CLAUDE.md) : copier dans le dépôt les jeux SD de
  Laserworld (« Standard Fileset 2025 », « Extended Set »), ou écrire un
  convertisseur ciblant leur contenu.
- **Pratique** : il manque une notion de **banque SD** (quel cue = quel
  numéro), exportée en un dossier prêt à copier, et réutilisée par la
  sortie Art-Net → T-302.

---

## 4. Contacts Laserworld pour relancer

| Canal | Détail | Source |
|---|---|---|
| Programme SDK | « Norbert », formulaire/e-mail via la page API (e-mail masqué en JS) | [page API/SDK](https://www.laserworld.com/en/software/more-shownet-compatible-software/shownet-api-sdk.html) |
| Siège | **Laserworld AG**, Kreuzlingerstrasse 5, CH-8574 Lengwil, Suisse. Tél. +41 71 677 80 80, hotline service +41 71 677 80 99, lun–ven 9 h–17 h ; anglais et allemand | [contact](https://www.laserworld.com/en/about-laserworld/contact.html), [mentions légales](https://www.laserworld.com/en/imprint.html) |
| Formulaire de contact | laserworld.com/en/about-laserworld/contact | idem |
| Service après-vente (Allemagne) | +49 8020 451 99 77 ; dossiers SAV/RMA en ligne ; outil « Quick Support » | [service center](https://www.laserworld.com/en/support-and-service/service-center.html) |
| Lignes régionales | **Français : +34 910 059 459** ; UK +44 161 872 0272 ; Pologne +48 660 614 593 ; Turquie +90 533 377 0010 | [contact](https://www.laserworld.com/en/about-laserworld/contact.html) |
| Forum Showeditor (le staff y répond) | section « ShowNET Konfiguration / Configuration » | [exemple de fil](https://www.showeditor.com/en/forum/shownet-configuration/162-probleme-de-reconnaissance-shownet.html) |
| Réseaux sociaux | @laserworldag (LinkedIn, X, Facebook, Instagram, YouTube) | [contact](https://www.laserworld.com/en/about-laserworld/contact.html) |
| Revendeur | le magasin qui a vendu le ShowNET (accès « Dealer Login » chez Laserworld) | idem |

Aucun bureau Laserworld USA n'apparaît sur la page contact.

**Brouillon de relance** (en anglais, à envoyer au formulaire + au
siège + à la ligne française, en citant le premier échange) :

> Subject: ShowNET API/SDK – integration partner request (Laser Studio, open source)
>
> Hello Norbert, hello Laserworld team,
> Following up on our exchange of <date> about the ShowNET API/SDK. Laser
> Studio is a publicly available laser show application for macOS
> (Apple Silicon), open source on GitHub (<repo URL>). It currently outputs to
> IDN/Ether Dream and, through MadMapper, to ShowNET. We would like native
> ShowNET output via your official library and are ready to sign the NDA;
> keys would stay outside the public repository. Legal address: <…>.
> Our ShowNET: external interface, firmware 2016050502. Two questions:
> (1) can you send the NDA and SDK? (2) does updating to firmware 2025120902
> keep the licence codes stored on the unit?
> Thank you, <name>

---

## 5. Repli matériel : DAC ouverts compatibles macOS + `laser-dac`

Le ShowNET externe se branche aux lasers par un **câble ILDA DB25**. N'importe
quel DAC ILDA peut prendre sa place sur ce câble, sans toucher aux lasers.

| DAC | Prix | Liaison | ILDA DB25 | Feature `laser-dac` | État chez nous | Source |
|---|---|---|---|---|---|---|
| **HeliosPRO** | 219 $ / **185 €**, port offert (expédié de Norvège) | Ethernet (**IDN**) + USB-C ; lecteur `.ild` hors ligne, ~7 Go | oui (femelle) | `idn` | **marche déjà** (feature activée dans `studio/Cargo.toml`) — à valider sur l'appareil | [Bitlasers HeliosPRO](https://bitlasers.com/heliospro-laser-dac/) |
| **Helios** (USB) | 114 $ / **99 €**, port offert | USB, 12 bits, 65,5 kpps, sans pilote sur Mac ; SDK MIT | oui | `helios` (dépend de `rusb`/libusb) | à activer (T-303) | [Bitlasers Helios](https://bitlasers.com/helios-laser-dac/), [GitHub](https://github.com/Grix/helios_dac) |
| Helios + adaptateur **OpenIDN** | 109 $ + 109 $ | Ethernet/Wi-Fi (IDN), firmware OpenIDN (Univ. Bonn) | oui (via Helios) | `idn` | marche déjà | [Bitlasers OpenIDN](https://bitlasers.com/openidn-network-adapter-for-the-helios-dac/), [PhotonLexicon](https://photonlexicon.com/forums/showthread.php/29143-New-OpenIDN-DAC-adapter) |
| **Ether Dream 4** | 229 $ (nu) / 289 $ (boîtier) ; 249 $ chez MotionLasers/Innolasers, lien Europe sur xlaser.com | Ethernet | oui | `ether-dream` | **marche déjà** | [X-Laser](https://www.xlaser.com/products/ether-dream-4), [MotionLasers](https://www.motionlasers.com/products/software/etherdream-4) |
| **StageMate ISP** (DexLogic, Allemagne) | sur devis | Ethernet **IDN**, ISP-DB25 + ISP-DMX, 16 bits X/Y | oui | `idn` | marche déjà (IDN standard) | [DexLogic](http://www.dexlogic.de/work/4108-isp/StageMate-ISP-en.html) |
| LaserCube (Wicked Lasers) | — | USB / Wi-Fi | entrée/sortie ILDA **seulement sur les Ultra** ; c'est un projecteur, pas une interface pour d'autres lasers | `lasercube-*` | hors sujet ici | [manuel CUBE2](https://www.laseros.com/CUBE2-CUBE2ULTRA.manual-20230404.pdf) |

Notes :
- Les features de `laser-dac` 0.13.1 : `helios`, `ether-dream`, `idn`,
  `lasercube-network`, `lasercube-usb`, `avb`, `oscilloscope`, et un
  `receiver` IDN (lu dans le `Cargo.toml` de la crate).
- Limites Helios : 4095 points/frame en USB, 8192 en IDN
  (`madmapper-and-open-content.md`).
- En Europe, Bitlasers expédie gratuitement depuis la Norvège : attention aux
  frais de douane/TVA à l'import dans l'UE (non précisés sur le site).
- Le HeliosPRO a aussi un **lecteur `.ild` hors ligne**, comme la carte SD du
  ShowNET, mais avec firmware et SDK ouverts.

---

## 6. Projets open source autour du ShowNET

- **Aucun projet public** trouvé qui pilote le ShowNET par une voie
  officielle en publiant le code : le NDA et les clés par partenaire
  l'empêchent. Les intégrations existantes (ILD Render, Modulaser, CloudLase,
  MadMapper, TouchDesigner) sont fermées.
- **Voies ouvertes et permises** : la définition DMX de l'Open Fixture
  Library ([fixture](https://open-fixture-library.org/laserworld/shownet),
  [JSON](https://github.com/OpenLightingProject/open-fixture-library/blob/master/fixtures/laserworld/shownet.json),
  modes DJ 19 can. et Pro 34 can., 2020) ; PONK (Apache-2.0) et
  `ponk-protocol` (MIT) pour passer par MadMapper/TouchDesigner.
- **Rétro-ingénierie** : je n'ai trouvé **aucun dépôt public** qui
  rétro-ingénierie le protocole ShowNET (recherches GitHub et web). Un fil du
  forum openFrameworks pose la question (« LaserWorld lasers and showNET
  protocols », [lien](https://forum.openframeworks.cc/t/laserworld-lasers-and-shownet-protocols/37296),
  page inaccessible, 403). Quoi qu'il en soit, CLAUDE.md l'interdit : on ne
  l'utilise pas.

---

## 7. Options classées

Échelle : faisabilité (★ = douteux, ★★★ = sûr), coût matériel/logiciel,
statut légal, effort de dev.

| # | Option | Faisabilité | Coût | Légal | Effort | Verdict |
|---|---|---|---|---|---|---|
| 1 | **Sortie PONK → MadMapper/MadLaser → ShowNET** (T-300) | ★★★ (MadMapper 5.2+ reçoit PONK ; le ShowNET marche déjà avec MadMapper chez l'utilisateur) | 0 € si MadLaser déjà licencié, sinon 199 € HT ou 19 €/mois | ✅ protocole ouvert Apache-2.0, logiciel sous licence officielle | **Faible** (une `Output`, crate MIT existante) | **À faire maintenant** |
| 2 | **DAC ouvert à la place du ShowNET** : HeliosPRO (IDN) ou Ether Dream 4 — zéro code ; Helios USB — T-303 | ★★★ | 99–290 € | ✅ SDK/protocoles ouverts | **Nul** (IDN/Ether Dream), faible (Helios USB) | **À acheter** (HeliosPRO conseillé) : sortie directe sans MadMapper ni Laserworld |
| 3 | **SDK officiel Laserworld** → `ShowNetOutput` natif | ★ (pas de réponse ; dépend d'eux) | 0 € connu | ✅ sous NDA | Moyen une fois reçu | **Relancer** (§4) ; tâche à créer à la réception |
| 4 | **Art-Net → lecture SD du ShowNET** (T-301 + banque SD T-302, sur T-001/T-011) | ★★ (firmware à mettre à jour, DHCP, PC Windows pour l'Admin Tool ; comportement en perte de signal inconnu) | 0 € (+ PC Windows ponctuel) | ✅ documenté publiquement | Moyen | **Mode autonome** utile en installation fixe ; pas un remplaçant du streaming |
| 5 | PC/VM Windows + Showcontroller LIVE piloté par notre studio en Art-Net/MIDI (licence Showcontroller sur le boîtier) | ★★ | PC Windows | ✅ usage normal du logiciel | Moyen (T-013 côté sortie) | Plan C seulement : lourd, et Showcontroller fait alors le rendu, pas nous |
| 6 | Autres ponts : TouchDesigner (PONK + ShowNET, support Mac du ShowNET non documenté), Modulaser (MIDI/OSC), CloudLase (aucune entrée documentée) | ★ | licences | ✅ | Variable | Pas prioritaire |
| ✗ | Rétro-ingénierie / déchiffrement du protocole ShowNET | — | — | ❌ **interdit** (CLAUDE.md) | — | **Exclu** |

---

## 8. Plan recommandé

1. **Tout de suite (dev)** : **T-300 sortie PONK**. L'utilisateur garde son
   ShowNET et sa licence MadMapper ; Laser Studio fait le rendu, MadMapper sert
   de pont. Réglages MadLaser à documenter (désactiver ses optimisations via
   les métadonnées PONK pour ne pas re-densifier nos tracés).
2. **Tout de suite (achat, utilisateur)** : un **HeliosPRO** (185 €) branché
   sur le câble ILDA d'un laser. Il marche avec notre sortie IDN existante :
   une vraie sortie directe, sans MadMapper ni Laserworld. **T-303** ajoute le
   Helios USB et un sélecteur de sortie dans l'UI.
3. **En parallèle (utilisateur)** : relancer Laserworld par 3 canaux (page
   SDK, siège, ligne française) avec le brouillon du §4. Demander aussi si la
   mise à jour du firmware garde les licences.
4. **Ensuite (dev, optionnel)** : **T-302 banque SD** puis **T-301 sortie
   Art-Net ShowNET** pour un mode autonome / installation fixe, une fois le
   firmware mis à jour et l'Art-Net vérifié par l'utilisateur.
5. **À réception du SDK** : créer la tâche `ShowNetOutput` (clés et
   bibliothèque hors dépôt, même contrat `Output` : armement, blackout,
   `blank_now`).

Dans tous les cas, les règles de sécurité restent : laser désarmé au
démarrage, Échap = blackout, et **aucun test ni agent** n'envoie de PONK vers
un MadMapper réel, d'Art-Net vers 192.168.129.51 ou de flux vers un DAC réel.
Les tests utilisent des récepteurs locaux sur `127.0.0.1` et un port libre,
**jamais le port 5583 ni le multicast** : un MadMapper ouvert sur la même
machine les recevrait.
