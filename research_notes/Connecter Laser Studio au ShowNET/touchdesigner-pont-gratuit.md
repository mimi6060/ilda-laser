# TouchDesigner comme pont gratuit PONK → ShowNET (Laser Studio → ShowNET)

> Portée : est-ce que la chaîne « app externe (Laser Studio, émet du PONK) → TouchDesigner (licence Non-Commercial gratuite) → Laserworld ShowNET » est un pont **fonctionnel, gratuit et légal** pour sortir sur le ShowNET sans le blackout de démo de MadMapper, et comment la monter exactement.
> Recherche faite le 2026-10-21. Tout vient de sources publiques (docs/forum Derivative, GitHub MadMapper/Ponk, pages Laserworld, forum Liberation Laser).

---

## Q1 (LE DÉCIDEUR) — La licence Non-Commercial autorise-t-elle de charger un Custom Operator C++ précompilé tiers (le plugin PONK de MadMapper) ?

### Takeaway
**Oui, selon la source la plus autoritaire.** Depuis TouchDesigner build 2019.10000, Derivative a explicitement retiré la limite de licence sur les opérateurs C++ : « C++ operators now run on all license types including Non-Commercial ». Les Custom Operators (plugins précompilés déposés dans le dossier Plugins, qui apparaissent sous leur propre nom dans le menu OP Create) sont la forme « finale » de ces plugins C++ et relèvent de la même annonce. **Il reste une contradiction dans la doc de Derivative** (une page produit liste encore « C++ OPs » parmi les exclusions Non-Commercial) et **je n'ai trouvé aucun rapport d'utilisateur confirmant précisément le plugin PONK tournant sur Non-Commercial** : à valider par un test pratique, mais la probabilité est forte que ça charge.

### Cited Findings
- Annonce officielle, page « What's New in 099 2019.10000 » : « We are also excited to announce that **C++ operators now run on all license types including Non-Commercial.** » — [Derivative: What's New in 099 2019.10000](https://docs.derivative.ca/What's_New_in_099_2019.10000)
- Même page, motivation des Custom OPs : « When removing the license limitations for C++ plugins » (les Custom OPs ont été introduits en même temps que la levée des limites de licence sur les plugins C++). — [Derivative: What's New in 099 2019.10000](https://docs.derivative.ca/What's_New_in_099_2019.10000)
- Distinction Custom OP vs CPlusPlus OP : un Custom OP est un opérateur tiers construit avec l'API C++ qui se comporte comme un opérateur natif ; « very similar to the CPlusPlus TOP, CPlusPlus CHOP, CPlusPlus SOP, and CPlusPlus DAT operators », mais « have their own unique names and appear in the OP Create menu for better integration in TouchDesigner ». — [Derivative: Custom Operators](https://docs.derivative.ca/Custom_Operators)
- Confirmation tierce (notes de version du plugin communautaire TD-OpenCV3TOP) : une branche expérimentale « implemented a new way of dealing with C++ plugins, and it allowed users to utilize C++ plugins without a Commercial or Pro License ». — [GitHub: TD-OpenCV3TOP](https://github.com/mourendxu/TD-OpenCV3TOP)
- **Contradiction** : la page produit liste encore les « C++ OPs » comme exclus du Non-Commercial : « does not include a subset of Pro/Commercial Operators (such as **Shared Memory OPs, C++ OPs, SDI OPs**, etc) ». — [Derivative: TouchDesigner Products](https://docs.derivative.ca/TouchDesigner_Products)
- Mais une autre version de la page Non-Commercial liste des exclusions **sans mentionner les C++ OPs** : « does not include a subset of Pro/Commercial Operators (such as Shared Memory OPs, DirectX OPs, Leuze ROD4 CHOP etc) ». — [Derivative: TouchDesigner Non-Commercial](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial)
- Mécanisme de chargement (indépendant de la licence) : « Custom OPs are used by simply placing the plugins in the correct location on disk. They will be detected when TouchDesigner starts. » Au premier chargement, TD demande une permission et stocke une signature du binaire dans un `.json`. — [Derivative: Custom Operators](https://derivative.ca/UserGuide/Custom_Operators)

### Inferences
- La mention « C++ OPs » encore présente sur la page Produits est très probablement un vestige obsolète d'avant 2019 ; l'annonce datée et explicite de 2019.10000 fait foi et est cohérente avec le fait que l'écosystème (plugins OpenCV, etc.) tourne sur Non-Commercial depuis.
- Le plugin PONK étant distribué comme Custom Operator C++ (voir Q2), il devrait donc se charger sur Non-Commercial comme n'importe quel plugin C++ ; le dialogue de permission au premier lancement est un comportement de sécurité normal, pas une restriction de licence.

### Gaps
- Aucune confirmation « hands-on » trouvée d'un utilisateur faisant tourner spécifiquement le **plugin receiver PONK de MadMapper** sous TouchDesigner Non-Commercial. La contradiction de doc n'est pas levée par Derivative noir sur blanc pour ce plugin précis → **un test réel reste nécessaire** (charger le plugin dans une install Non-Commercial et vérifier qu'il apparaît dans OP Create sans erreur de licence).

---

## Q2 — Comment fonctionne le plugin PONK pour TouchDesigner (nature, install, UDP, version TD) ?

### Takeaway
PONK (« Path Over Network ») est un protocole ouvert (Apache-2.0) qui transporte des chemins 2D colorés (trajectoires laser) entre logiciels via UDP. L'intégration TouchDesigner est écrite en C++, livrée gratuitement sur GitHub pour macOS et Windows, et contient un **PonkSender** et un **PonkReceiver**. Le détail d'install exact (dossier Plugins, nom d'opérateur, port UDP par défaut) n'est **pas confirmé par les sources publiques consultées** et doit être lu dans le dossier `Touch Plugin` du dépôt.

### Cited Findings
- PONK « has been developped to transfer laser path from a software to another over network using UDP » ; l'intégration TD est « implemented in C++ », échange des chemins laser entre TouchDesigner et MadMapper (et entre plusieurs instances). Développé en étroite collaboration avec MadMapper et financé par eux ; « available for free on GitHub for both macOS and Windows ». — [Derivative community post: Ponk — Path Over Network for TouchDesigner](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438)
- Dépôt sous licence Apache-2.0 ; transport « over UDP multicast or unicast » ; les récepteurs « subscribe to the multicast group » ; `GEOM_UDP_MAX_DATA_BYTES_PER_PACKET` = 8192 par défaut. — [GitHub: madmappersoftware/Ponk](https://github.com/madmappersoftware/Ponk)
- Le dossier `Touch Plugin` contient `PonkReceiver`, `PonkSender` et un dossier `examples` ; un dossier `Common/Cpp` existe aussi. — [GitHub: madmappersoftware/Ponk — Touch Plugin](https://github.com/madmappersoftware/Ponk/tree/master/Touch%20Plugin)
- MadMapper 5.2 a ajouté le support PONK : « an opensource framework to share polylines over the network ». — [Derivative community post](https://derivative.ca/community-post/ponk-path-over-network-touchdesigner/74438)
- Côté usage réel : sur le forum Liberation Laser, un dev confirme « if you want to use Touch Designer the best option is to send 2D paths via PONK to Liberation » (preuve que PONK TD→autre soft est un flux établi). — [Liberation Laser forum: TouchDesigner content to Liberation via PONK](https://forum.liberationlaser.com/t/touchdesigner-content-to-liberation-via-ponk/2249)

### Inferences
- Les noms de dossiers `PonkSender`/`PonkReceiver` + `Common/Cpp` indiquent un Custom Operator C++ (code + binaire à déposer dans le dossier Plugins de TD). Pour la chaîne « app externe → TD », c'est le **PonkReceiver** qui compte : il reçoit l'UDP PONK et expose les chemins dans TD (très probablement comme CHOP/opérateur de type laser à brancher ensuite sur le Laser CHOP).
- Le port 127.0.0.1:5583 cité dans la consigne n'a **pas** pu être vérifié dans les sources ; PONK étant multicast par défaut, il faudra confirmer le port/adresse exacts dans le code du dépôt et s'assurer que Laser Studio émet vers le même port/mode (unicast 127.0.0.1 vs multicast).

### Gaps
- Emplacement d'install précis du plugin, nom exact de l'opérateur dans TD, **port UDP par défaut (5583 ?)**, mode (multicast vs unicast loopback) et version minimale de TouchDesigner requise : non trouvés dans les sources publiques. À lire directement dans `Touch Plugin/` (README/examples) et dans le code du PonkReceiver.
- Existe-t-il un binaire précompilé prêt à l'emploi dans les Releases GitHub, ou faut-il compiler soi-même ? Non confirmé (le dépôt semble fournir le source ; vérifier l'onglet Releases).

---

## Q3 — Sortie ShowNET native de TouchDesigner (Laser Device CHOP) : auto-détection LAN, licence/surcoût, plafonds Non-Commercial ?

### Takeaway
TouchDesigner a un **support ShowNET natif intégré** via le **Laser Device CHOP** (opérateur natif, pas un plugin C++). Il **auto-détecte** les cartes ShowNET sur le LAN, ne demande **aucune licence ni matériel supplémentaire** (annonce Laserworld), et le DAC embarqué du ShowNET est piloté directement par Ethernet. Laserworld indique que TD est gratuit pour un usage privé/non-commercial mais avec une **résolution de sortie limitée** sans licence ; Derivative ne chiffre pas ce plafond pour le laser dans les pages lues.

### Cited Findings
- « The Laser Device CHOP sends laser points to EtherDream, Helios, and ShowNET devices. For ShowNET, an onboard DAC is used instead of an ILDA cable. » — [Derivative: Laser Device CHOP](https://docs.derivative.ca/Laser_Device_CHOP)
- Mise en place : créer un Laser Device CHOP, choisir ShowNET comme type d'appareil, puis sélectionner l'unité dans le menu ; un Laser CHOP génère les points et alimente le Laser Device CHOP. — [Derivative: Laser Device CHOP](https://docs.derivative.ca/Laser_Device_CHOP)
- Pipeline : « a CHOP, POP or SOP defining the shapes is sent to the Laser CHOP », qui « processes the data into streams of samples », puis passe au Laser Device CHOP. — [Derivative: Lasers](https://derivative.ca/UserGuide/Lasers)
- Auto-détection + pas de surcoût (annonce Laserworld) : TD « automatically detects ShowNET mainboards within the network » ; un simple LAN suffit, « could be as simple as a direct LAN cable » ; la connexion « eliminates the need for additional licenses or extra hardware components » ; jusqu'à 40 cartes ShowNET par réseau physique (plus avec plusieurs LAN/VLAN). — [Laserworld: TouchDesigner supports direct output to ShowNET laser mainboards](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)
- Page compatibilité Laserworld (via snippet de recherche) : « TouchDesigner is free to use for private and non-commercial projects, but has **limited output resolution if no license is purchased**. » — [Laserworld: ShowNET compatible — TouchDesigner](https://www.laserworld.com/en/shownet-compatible/touchdesigner)
- Thread communautaire dédié au support ShowNET dans TD (existence confirmée). — [Derivative community post: ShowNET laser support](https://derivative.ca/community-post/shownet-laser-support/68339)
- Limite d'image Non-Commercial : « image resolution is limited to 1280x1280 » (sauf images 1–2 px de haut). — [Derivative: TouchDesigner Non-Commercial](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial)

### Inferences
- Le Laser Device CHOP étant un opérateur **natif/built-in** (il figure dans « Includes all basic operators », coché pour les 4 types de licence sur la page Licensing), la sortie ShowNET elle-même n'est pas bloquée par le Non-Commercial. La seule limite probable est la « résolution de sortie » évoquée par Laserworld.
- La « limited output resolution » mentionnée par Laserworld ne correspond pas forcément au plafond image 1280×1280 (le laser est vectoriel, pas raster) ; c'est probablement un plafond sur le nombre de points/samples par trame côté sortie laser en Non-Commercial, mais **aucune valeur chiffrée** n'a été trouvée. Pour du vectoriel de show, ce plafond pourrait être acceptable — à vérifier concrètement.

### Gaps
- Valeur exacte du plafond de « output resolution » laser en Non-Commercial (points/sample par frame, kpps effectifs) : non trouvée. Seule la limite image raster 1280×1280 est chiffrée, et elle ne concerne pas directement la sortie vectorielle laser.
- Version minimale de TouchDesigner embarquant la sortie ShowNET : non précisée (support relativement récent ; prendre une build récente).

---

## Q4 — Si le plugin PONK est bloqué en Non-Commercial : autre moyen GRATUIT de faire entrer des trames laser dans TouchDesigner ?

### Takeaway
Oui, plusieurs chemins de secours **100 % natifs** (donc indépendants de toute question de licence C++) : faire émettre à Laser Studio un format que TD lit avec des opérateurs built-in — **OSC CHOP**, **UDP brut dans un DAT/CHOP**, ou données de forme construites dans TD — puis router vers le Laser CHOP → Laser Device CHOP → ShowNET. L'important : ces opérateurs d'entrée sont natifs et le Laser Device CHOP l'est aussi, donc le doute du décideur Q1 disparaît.

### Cited Findings
- Le Laser CHOP accepte des formes depuis « a CHOP, POP or SOP » : n'importe quelle source native produisant des points X/Y/couleur peut alimenter la chaîne laser. — [Derivative: Lasers](https://derivative.ca/UserGuide/Lasers)
- TouchDesigner expose des entrées réseau natives standard (OSC CHOP/DAT, UDP/TCP DAT) utilisées couramment pour injecter des données externes (famille d'opérateurs documentée). — [Derivative: Operator Families](https://docs.derivative.ca/Operator)
- MadMapper rend ses protocoles publics pour qu'il soit facile d'envoyer du contenu depuis TouchDesigner (contexte : l'écosystème favorise ces ponts ouverts). — mentionné dans [Derivative tag: touchdesigner](https://derivative.ca/tags/touchdesigner)

### Inferences
- Chemin de secours recommandé si jamais le plugin PONK refuse de charger : faire émettre par Laser Studio un flux **UDP/OSC** décrivant les points (X,Y dans -1..1 ; R,G,B dans 0..1), le recevoir via **OSC CHOP** ou **UDP In DAT** (puis convertir en CHOP), reconstruire les polylignes et les envoyer au **Laser CHOP**. C'est plus de travail côté Laser Studio (nouveau format de sortie) que PONK, mais ça évite tout opérateur C++.
- Syphon/Spout (texture raster) existe mais est inadapté au vectoriel laser (il faudrait un passage raster→chemins), donc à éviter pour ce besoin ; privilégier un flux de points (OSC/UDP) ou le PONK natif.
- Comme Laser Studio émet **déjà** du PONK (vers MadMapper), le chemin PONK→TD reste le moins coûteux à mettre en œuvre ; le repli OSC/UDP n'est à développer que si Q1 échoue au test.

### Gaps
- Pas de `.tox` tout-fait public « PONK receiver pur TD sans plugin C++ » trouvé. Il faudrait soit le plugin PONK, soit développer un décodeur du format PONK en Python/GLSL dans TD à partir d'un UDP In DAT (le format binaire PONK est documenté dans le dépôt GitHub — réalisable mais non trivial).

---

## Q5 — Légalité de licence : Non-Commercial vs Commercial, ce qui compte comme usage non-commercial

### Takeaway
TouchDesigner Non-Commercial est **gratuit et légal uniquement pour un usage non rémunéré** (perso, apprentissage, évaluation, shows gratuits). Dès que l'utilisateur ou son organisation est **payé/rémunéré** pour un travail impliquant TouchDesigner (gig payant, prestation client), une licence **Commercial ou Pro** est obligatoire. Les clés Non-Commercial 2021+ n'expirent pas.

### Cited Findings
- « Non-Commercial licenses are for **personal, educational or evaluation purposes for non-paying/non-commercial use only**. » — [Derivative: Licensing](https://derivative.ca/UserGuide/Licensing)
- « You may not use TouchDesigner Non-Commercial for any **paid work** with TouchDesigner. » — [Derivative: Licensing](https://derivative.ca/UserGuide/Licensing)
- Test de qualification : « You are not receiving money or compensation for your work with TouchDesigner. » — [Derivative: TouchDesigner Non-Commercial](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial)
- « If you or your organization are **compensated for your work** with TouchDesigner, you must have a Commercial or Pro license. » — [Derivative: Licensing](https://derivative.ca/UserGuide/Licensing)
- Validité des clés : « Non-Commercial keys have no time limit when using TouchDesigner from 2021 or later. When using 2020 or earlier builds, Non-Commercial keys work for 1 year from the date they are created. » — [Derivative: TouchDesigner Non-Commercial](https://derivative.ca/UserGuide/TouchDesigner_Non-Commercial)
- Option Éducation : étudiants/enseignants d'établissements accrédités peuvent demander une licence Éducation (fonctions = Commercial, mais usage non rémunéré). — [Derivative: Licensing](https://derivative.ca/UserGuide/Licensing)

### Inferences
- Pour l'utilisateur : développer/tester Laser Studio chez lui, faire des shows gratuits, apprendre → Non-Commercial légitime. Un **spectacle payé** (client, cachet, événement commercial) → il lui faudrait une licence Commercial/Pro de TouchDesigner (payante), indépendamment du fait que Laser Studio soit open-source et gratuit.
- Le fait que Laser Studio soit open-source/gratuit ne « contamine » pas la licence TD : c'est l'usage que fait l'opérateur de TD (payé ou non) qui détermine le tier requis.

### Gaps
- Zone grise (ni rémunéré ni purement perso, ex. association, show de charité avec défraiement) : Derivative renvoie à son User Agreement ; le texte exact de l'accord n'a pas été récupéré. En cas de doute, contacter Derivative.

---

## Q6 — Montage pas à pas de la chaîne + pièges

### Takeaway
Chaîne cible : **Laser Studio (émet PONK) → plugin PonkReceiver dans TouchDesigner → Laser CHOP → Laser Device CHOP (ShowNET) → projecteur**. Toutes les briques TD existent et sont gratuites ; le seul point à vérifier par un test est le chargement du plugin PONK en Non-Commercial (Q1).

### Cited Findings / étapes (dérivées des docs citées ci-dessus)
1. **Installer TouchDesigner Non-Commercial** (gratuit), build récente incluant le support ShowNET. Clé Non-Commercial sans expiration en 2021+. — [Derivative: Licensing](https://derivative.ca/UserGuide/Licensing)
2. **Installer le plugin PONK** : récupérer le dossier `Touch Plugin` de [github.com/madmappersoftware/Ponk](https://github.com/madmappersoftware/Ponk), déposer le binaire du **PonkReceiver** dans le dossier Plugins de TD — macOS : `/Users/<user>/Library/Application Support/Derivative/TouchDesigner099/Plugins` ; Windows : `Documents/Derivative/Plugins` ; ou un dossier `Plugins` à côté du `.toe`. Redémarrer TD ; accepter le dialogue de permission au 1er chargement. — [Derivative: Custom Operators](https://derivative.ca/UserGuide/Custom_Operators)
3. **Émettre le PONK depuis Laser Studio** vers l'adresse/port attendus par le PonkReceiver (loopback `127.0.0.1` si TD est sur la même machine ; sinon IP LAN). Vérifier port/mode (unicast vs multicast) dans le code du dépôt. C'est le même mécanisme PONK que Laser Studio utilise déjà vers MadMapper.
4. **Router dans TD** : brancher la sortie du PonkReceiver (chemins/points) vers un **Laser CHOP** qui transforme en samples. — [Derivative: Lasers](https://derivative.ca/UserGuide/Lasers)
5. **Sortie ShowNET** : créer un **Laser Device CHOP**, type d'appareil = **ShowNET**, sélectionner l'unité auto-détectée sur le LAN. — [Derivative: Laser Device CHOP](https://docs.derivative.ca/Laser_Device_CHOP)
6. **Réseau** : LAN filaire (un câble direct suffit), pas de licence/matériel en plus, jusqu'à 40 ShowNET par réseau. — [Laserworld announcement](https://www.laserworld.com/en/newslist/5805-touchdesigner-supports-direct-output-to-shownet-laser-mainboards.html)

### Pièges à documenter
- **Dialogue de permission du plugin** : normal (sécurité), apparaît au 1er chargement et si le binaire change ; une signature est stockée dans un `.json` du dossier Plugins. — [Derivative: Custom Operators](https://derivative.ca/UserGuide/Custom_Operators)
- **Un seul logiciel contrôle un ShowNET à la fois** (contrainte du boîtier, cf. CLAUDE.md) : fermer MadMapper/Showeditor avant que TD ne prenne la main.
- **Port/mode PONK** : s'assurer que Laser Studio et le PonkReceiver sont sur le même port et le même mode (unicast loopback vs multicast). Non confirmé publiquement (Q2 gap).
- **Résolution de sortie limitée en Non-Commercial** (Laserworld) : à mesurer sur du contenu réel ; peut imposer de passer en Commercial pour un rendu haute densité. — [Laserworld: ShowNET compatible — TouchDesigner](https://www.laserworld.com/en/shownet-compatible/touchdesigner)
- **Sécurité** (règles projet) : ce pont ne change rien aux obligations — arrêt matériel réel (interlock/E-stop/clé), jamais compter sur un stop logiciel. Les tests/agents ne doivent jamais envoyer de paquet réel au ShowNET ni au MadMapper de l'utilisateur.

### Gaps
- Le port exact (5583 ?) et le nom précis de l'opérateur receiver dans TD restent à confirmer dans le dépôt → impossible de donner les valeurs exactes de l'étape 3 sans lire `Touch Plugin/`.

---

## Synthèse pour le rédacteur

- **Verdict décideur (Q1) : probablement OUI.** Depuis TD 2019.10000, les opérateurs/plugins C++ (dont les Custom Operators précompilés) tournent sur **toutes** les licences, Non-Commercial inclus — source officielle datée et explicite. Une page produit contredit (liste encore « C++ OPs » comme exclu) et aucun rapport utilisateur sur le plugin PONK précis n'a été trouvé → **à confirmer par un test pratique**, mais le pont est très vraisemblablement viable et gratuit.
- **La sortie ShowNET elle-même (Laser Device CHOP) est native, auto-détectée, sans surcoût, et non bloquée par le Non-Commercial** ; seule une « résolution de sortie limitée » (non chiffrée) s'applique en gratuit.
- **Légalité** : gratuit et légal pour usage non rémunéré ; un show payé exige une licence TD Commercial/Pro (payante), que Laser Studio soit open-source ou non.
- **Filet de sécurité** si le plugin PONK refuse de charger : entrée native OSC/UDP dans TD (Laser Studio devrait alors émettre un flux de points OSC/UDP) → Laser CHOP → Laser Device CHOP. Plus de dev côté Laser Studio, mais aucun opérateur C++ en jeu.
