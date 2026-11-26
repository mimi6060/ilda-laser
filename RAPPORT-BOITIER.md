# Rapport d'audit sécurité — Communication réseau avec le boîtier laser

**Projet :** chaîne de contrôle laser (MadMapper/MadLaser + Showcontroller ↔ Laserworld ShowNET / Moncha.NET)
**Portée :** découverte réseau et transport entre le poste de contrôle et le boîtier laser
**Date :** 2026-10-17
**Cadre :** test autorisé sur matériel et réseau de l'auteur (Mac `192.168.129.10`, sous-réseau `192.168.129.0/24`)
**Méthode :** observation passive (lsof, mDNS), capture réseau sur `en0` (tcpdump), lecture de la documentation publique

> ⚠️ **Avertissement IP / NDA (règle dure du projet).**
> Aucun secret n'a été extrait pendant cet audit. Le **flux de contrôle « live » du ShowNET est chiffré** ; sa clé est sous NDA et **ne doit jamais être recherchée, capturée, rejouée ni réimplémentée**. Cet audit porte **uniquement sur la couche de découverte/annonce en clair** observée sur le réseau, pas sur le flux chiffré.

---

## 1. Synthèse exécutive

Il faut distinguer **deux couches** dans la communication avec le boîtier :

1. **Couche de découverte / annonce (en clair, NON authentifiée).** Le poste diffuse en broadcast UDP, en permanence et en clair, des requêtes de découverte (`Search Moncha.NET`, IDN-Hello). N'importe quel appareil du réseau les reçoit. **C'est le point faible observé.**
2. **Couche de contrôle « live » du ShowNET (CHIFFRÉE).** Le flux de points vers le ShowNET est chiffré par Laserworld ; l'accès légitime passe par l'API officielle sous NDA. **Cette couche n'a pas été analysée et ne doit pas l'être.**

**Conséquence pratique :** un attaquant déjà présent sur le réseau peut **trivialement détecter** le logiciel et le boîtier (ils s'annoncent en clair), mais il ne récupère **pas** la clé du flux chiffré par simple écoute — cette clé n'est pas sur le réseau. La barrière réelle reste donc **l'accès au réseau**, et **l'isolation réseau est la protection principale**.

---

## 2. Architecture observée

```
┌─────────────────────┐   découverte UDP broadcast (EN CLAIR)   ┌────────────────────────┐
│  Poste de contrôle  │ ───────────────────────────────────────▶ │  Boîtier laser         │
│  (Mac 192.168.129.10)                                           │  Laserworld ShowNET    │
│  - MadMapper/MadLaser │  contrôle « live » (CHIFFRÉ, via API    │  (DHCP, vu ~.51)       │
│  - Showcontroller     │   officielle sous NDA — hors périmètre) │                        │
└─────────────────────┘ ◀─────────────────────────────────────── └────────────────────────┘
        │
        │ loopback HTTP (127.0.0.1)  ← serveur local « laser-studio » (dev, preview)
        ▼
   interface web locale
```

Note : le ShowNET ne parle **aucun protocole ouvert** pour les points live (ni IDN, ni Ether Dream) ; son Art-Net ne transporte que du DMX. Le flux de points légitime passe par MadMapper (intégration ShowNET licenciée) ou par l'API officielle.

---

## 3. Ce que le poste émet réellement sur le réseau (couche découverte)

Capture `tcpdump -i en0 -nA 'udp and src 192.168.129.10 and dst 192.168.129.255'`.
Tous les paquets partent vers l'**adresse de broadcast** `192.168.129.255` → reçus par **chaque** appareil du sous-réseau.

| Port dest. | Taille | Contenu (ASCII) | Rôle | Cadence |
|---|---|---|---|---|
| **25000** | 18 o | **`Search Moncha.NET`** | Découverte du boîtier Laserworld | ~1 / 1,5 s |
| **7255** | 4 o | binaire | **IDN-Hello** (ILDA Digital Network, scan) | ~1 / s |
| **41855** | 30 o | binaire | Beacon de découverte secondaire | périodique |
| **45457** | 1 o | keepalive | Heartbeat | ~2 / s |

**Preuve (payload en clair) :**
```
IP 192.168.129.10.58854 > 192.168.129.255.25000: UDP, length 18
......a...$.Search Moncha.NET
```

Interprétation : le poste **diffuse en clair, à tout le réseau**, « y a-t-il un boîtier Laserworld ici ? ». Le boîtier répond → détection instantanée. **Cette couche n'échange aucune preuve d'identité.** Ce sont des balises (1 à 30 octets) : elles ne portent pas de secret.

> Note méthodo : les premières captures sur `-i any` (PKTAP) renvoyaient « 0 captured / N received by filter » — artefact macOS où les filtres de port échouent sur l'interface `any`. La capture correcte se fait sur l'interface réelle (`-i en0`, link-type EN10MB).

---

## 4. Constats de sécurité

### 4.1 🔴 Critique — Découverte en clair et non authentifiée
- `Search Moncha.NET` et IDN-Hello en **broadcast/UDP non authentifié**, nom du produit lisible en clair.
- **Conséquence** : énumération triviale par écoute passive ; un attaquant sur le LAN sait immédiatement qu'un contrôleur laser est présent, sans rien envoyer.

### 4.2 🟠 Moyen — Usurpation possible au niveau découverte
- Rien ne valide l'origine d'une réponse à `Search Moncha.NET` ni d'une annonce IDN-Hello.
- **Conséquence** : un appareil malveillant du LAN peut répondre et se présenter comme un boîtier (spoofing de la phase de découverte), ou leurrer un logiciel qui cherche son matériel.
- **Limite** : cela ne donne pas accès au flux de contrôle chiffré du ShowNET ; l'impact dépend de ce que le logiciel accepte d'une source non vérifiée.

### 4.3 🟢 Point fort — Le flux de contrôle live est chiffré
- Le flux de points du ShowNET est **chiffré** (Laserworld) ; la clé n'est pas exposée sur le réseau et l'accès légitime passe par l'API sous NDA.
- **Conséquence** : la simple écoute réseau ne permet pas de rejouer un show ni de piloter le laser via le flux officiel.

### 4.4 ℹ️ Clarification — « clé API » pour parler au boîtier
- **Couche découverte** : aucune clé, aucune authentification (c'est voulu par les protocoles).
- **Couche contrôle ShowNET** : chiffrée, **clé sous NDA** — obtenue légalement via l'API officielle, **jamais** par capture/rétro-ingénierie (règle dure du projet et contrainte légale).
- Il n'existe donc pas de « clé à sniffer » : la seule clé réelle est celle, sous NDA, du flux chiffré, et elle n'est pas récupérable par écoute.

---

## 5. Réponse à la question d'origine

> *« À quel point est-il facile pour un attaquant d'obtenir les infos d'API et d'authentification pour se connecter au boîtier sur le réseau ? »*

- **Repérer le logiciel et le boîtier : trivial** — ils s'annoncent eux-mêmes en broadcast clair.
- **Rejouer/piloter via le flux officiel : non trivial** — ce flux est chiffré (clé sous NDA, pas sur le réseau).
- **Barrière réelle : l'accès au réseau.** Sur le LAN, l'attaquant énumère sans effort, mais ne récupère pas la clé du flux chiffré par simple écoute.

Autrement dit : la **confidentialité du contenu** (le flux live) est protégée par le chiffrement vendeur ; la **présence et la topologie** (qui parle, à qui) ne le sont pas du tout.

---

## 6. Constat annexe (serveur de dev local `laser-studio`)

Hors canal boîtier, relevé pendant l'audit :

- **Stockage en `/tmp`** : instance de dev lancée avec `--data-dir /tmp/studio-test`, dossier monde-lisible (`drwxr-xr-x`), fichiers de conf en `0644` → lisibles par tout utilisateur/process local. 🟠 (poste de dev ; à éviter pour une config d'exploitation → `~/Library/Application Support/…`, `0600`).

  **Vérifié (2026-10-17)** — `ls -le /tmp/studio-test` :
  ```
  drwxr-xr-x@ 17 mac-m3-michel  wheel  544   /tmp/studio-test
  -rw-r--r--@  1 mac-m3-michel  wheel  ...   prefs.json
  -rw-r--r--@  1 mac-m3-michel  wheel  ...   safety_profile.json
  -rw-r--r--@  1 mac-m3-michel  wheel  ...   desks.json  grid.json  site.json  ...
  ```
  Le bit `r` pour « others » (dossier + fichiers) confirme l'accès en lecture par tout
  utilisateur local, y compris au profil de sécurité (`safety_profile.json`).
- **Point fort** : le serveur principal écoute sur `127.0.0.1` uniquement (non joignable depuis le LAN). 🟢
- **Point fort** : la télécommande tablette (`studio/src/remote.rs`) est bien conçue — pairing 6 chiffres, blocage après 5 essais, token de session `HttpOnly`/`SameSite=Strict` comparé en temps constant, périmètre d'actions restreint (ne peut ni armer, ni toucher sécurité/calibration). 🟢

> `pi-receiver/` est **abandonné** dans le projet (remplacé par le ShowNET) : non audité, aucune recommandation le concernant.

---

## 7. Recommandations (par priorité)

1. **🔴 Isolation réseau dédiée (prioritaire).** Boîtier + poste sur un **VLAN ou switch physique dédié**, **sans Wi-Fi** (Laserworld qualifie le Wi-Fi d'« unsafe operation »), sans route vers le réseau bureautique/invités/Internet. Le constat découverte-en-clair (§4.1) n'a alors plus d'exposition hors du segment laser.
2. **🔴 Interlock matériel indépendant du réseau.** Clé physique / E-stop / interlock projecteur, pour qu'aucune trame réseau ne puisse dépasser les limites de sécurité du laser. Ne jamais présenter un arrêt logiciel comme suffisant.
3. **🟠 Pare-feu sur le poste.** Restreindre les ports de découverte (25000, 7255, 41855, 45457) aux seules IP légitimes.
4. **🟠 Surveillance.** Journaliser/alerter sur une réponse inattendue à `Search Moncha.NET` (appareil usurpant un boîtier).
5. **🟢 Hygiène dev.** Pour `laser-studio` : data-dir hors `/tmp`, permissions `0600`, build `release` en exploitation.

Les protocoles de découverte n'étant pas authentifiables et le flux live étant déjà chiffré, **la défense utile se joue au niveau réseau et matériel**, pas applicatif.

---

## 8. Annexe A — commandes d'observation (reproduction)

```bash
# Ports réseau ouverts par l'application de contrôle
lsof -nP -a -p <PID_MadMapper> -i

# Ce que le poste diffuse en broadcast — interface RÉELLE, pas `any`
sudo tcpdump -i en0 -n 'udp and src 192.168.129.10 and dst 192.168.129.255'

# Idem avec contenu ASCII (révèle « Search Moncha.NET »)
sudo tcpdump -i en0 -nA 'udp and src 192.168.129.10 and dst 192.168.129.255'
```

> ⚠️ `tcpdump -i any` sur macOS (link-type PKTAP) fait échouer silencieusement les filtres de port (« 0 captured / N received by filter »). Toujours capturer sur l'interface réelle (`en0`).

---

## 9. Annexe B — démonstration passive de l'exposition (preuve reproductible)

Objectif : **prouver, par la seule observation**, que la présence du contrôleur laser
est annoncée en clair et reçue par n'importe quel acteur du réseau, **sans aucune
authentification**. Ces démonstrations **n'émettent rien vers le laser**, n'actionnent
aucun faisceau et **ne touchent pas** au flux de contrôle chiffré du ShowNET.

### Méthode A — un process local quelconque reçoit la découverte
Script fourni : `preuve-ecoute-passive.py` (écoute UDP seule, lecture seule).
```bash
python3 -I preuve-ecoute-passive.py
```
Résultat attendu :
```
11:14:45  de 192.168.129.10   vers :25000  (18 o)  '......a...$.Search Moncha.NET'
```
**Preuve** : ce process n'a présenté aucun identifiant, et reçoit pourtant l'annonce
du contrôleur laser en clair. (Si un port est déjà pris par l'appli, utiliser la Méthode B.)

### Méthode B — une *seconde* machine du réseau (preuve la plus forte)
Depuis un appareil qui n'est **ni** le poste de contrôle **ni** le boîtier, branché au même réseau :
```bash
sudo tcpdump -i <interface> -nA 'udp and (port 25000 or port 7255)'
# ou, sans root, le même preuve-ecoute-passive.py copié sur cette machine
```
Si cet appareil tiers reçoit `Search Moncha.NET`, c'est démontré :
**tout appareil présent sur le segment découvre le contrôleur laser, sans authentification.**
C'est l'incarnation littérale de l'attaquant passif.

### Ce que la démonstration établit — et ses limites
| Affirmation | Statut |
|---|---|
| La couche **découverte** est en clair et non authentifiée sur le LAN | ✅ **Prouvé** (capture `Search Moncha.NET`) |
| Un acteur non autorisé du réseau **voit et localise** le contrôleur laser | ✅ **Prouvé** (Méthode B) |
| Le **flux de contrôle live** est en clair | ❓ **Non prouvé** — jamais observé |
| Un attaquant peut **piloter le laser / afficher un message** | ❓ **Non prouvé** — non testé (NDA + sécurité laser) |

Honnêteté d'audit : l'impact « contrôle du faisceau » **n'est pas démontré** et n'a pas
été tenté — le vérifier exigerait d'analyser le flux chiffré du ShowNET (interdit : IP/NDA)
et de piloter un vrai laser hors chaîne de sécurité (risque physique). **La recommandation
d'isolation réseau ne dépend pas de cette preuve** : l'exposition prouvée en §4.1 la justifie
déjà pleinement, que le flux live soit chiffré ou non.

---

*Rapport généré dans le cadre d'un test de sécurité autorisé sur l'équipement et le réseau de l'auteur. Aucun secret n'a été extrait ; le flux chiffré du ShowNET n'a pas été analysé, conformément aux règles IP/NDA du projet. Les démonstrations fournies sont passives (observation seule) et n'actionnent aucun laser.*
