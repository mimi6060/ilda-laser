# Sortie PONK : Laser Studio → MadMapper/MadLaser → ShowNET

Le protocole du ShowNET est chiffré et le SDK Laserworld n'est pas encore
arrivé. En attendant, Laser Studio peut envoyer ses tracés à
**MadMapper/MadLaser**, qui pilote le ShowNET avec sa licence Laserworld
officielle. Le lien entre les deux est **PONK** (« Pathes Over NetworK »),
un protocole UDP ouvert publié par MadMapper (licence Apache-2.0,
[github.com/madmappersoftware/Ponk](https://github.com/madmappersoftware/Ponk)).
On ne touche ni au ShowNET ni à MadMapper : on leur envoie des tracés par un
protocole public, c'est tout.

```
Laser Studio ──PONK (UDP, port 5583)──▶ MadMapper/MadLaser ──▶ ShowNET ──▶ lasers
```

## Ce qu'il faut

- MadMapper **5.2 ou plus récent** avec l'extension **MadLaser** (licence ou
  location ; en essai gratuit, la sortie laser se coupe toutes les 120 s).
- Le ShowNET déjà configuré et visible dans MadMapper (comme aujourd'hui).
- MadMapper et Laser Studio sur le même Mac (le plus simple) ou sur le même
  réseau local.

## 1. Lancer Laser Studio avec la sortie PONK

La sortie PONK n'est **jamais** active par défaut : il faut la demander au
lancement.

```sh
# MadMapper sur ce Mac (127.0.0.1:5583)
cargo run -p laser-studio -- --ponk

# MadMapper sur un autre ordinateur du réseau
cargo run -p laser-studio -- --ponk 192.168.1.20:5583

# Multicast PONK (MadMapper 6 l'écoute), limité au réseau local
cargo run -p laser-studio -- --ponk 239.255.10.24:5583
```

- `--ponk` ne se combine ni avec `--device` ni avec `--test-output` : une
  seule sortie à la fois.
- En haut de l'interface, la pastille **PONK** et le texte
  « Sortie : PONK → MadMapper (127.0.0.1:5583) » confirment la sortie active.
- Le laser démarre **désarmé**, comme toujours. Tant qu'il est désarmé (ou
  après Échap / arrêt d'urgence), le studio envoie en continu, 60 fois par
  seconde, des **trames vides** : MadMapper n'affiche rien. Il ne faut jamais
  « couper » le flux, car MadMapper garde sinon la dernière image reçue.
- Ce qui part est la trame finale : calibration, zones de sécurité,
  limiteur de strobe, luminosité et armement déjà appliqués.
- L'identifiant d'émetteur est gardé dans `studio-data/ponk.json` : d'un
  lancement à l'autre, MadMapper retrouve la même source.

## 2. Recevoir le flux dans MadMapper

1. Ouvrir MadMapper, **sortie laser coupée** (bouton de sortie laser de
   MadLaser désactivé) pour la première mise en place.
2. Lancer Laser Studio avec `--ponk` (désarmé).
3. Une nouvelle **Live Source** « Laser Studio » apparaît dans la liste des
   médias de MadMapper (c'est ainsi que MadMapper présente les émetteurs
   PONK qu'il reçoit).
4. Créer (ou réutiliser) une **surface laser** reliée au ShowNET et lui
   donner ce média « Laser Studio ».
5. Dans Laser Studio, armer le laser (bouton LASER / Espace) : le dessin
   apparaît dans l'aperçu de MadMapper. Vérifier :
   - l'**échelle** : le studio envoie des coordonnées de -1 à 1 (tout le
     champ). Le dessin doit remplir la surface laser comme l'aperçu du
     studio. Sinon, ajuster la taille de la surface dans MadMapper ;
   - le **sens** : si le dessin est retourné (haut/bas ou gauche/droite),
     le corriger avec la surface MadMapper (rotation/miroir) ou avec la
     calibration de Laser Studio ;
   - les **couleurs**.
6. Seulement ensuite, activer la sortie laser de MadLaser.

Si rien n'arrive avec `127.0.0.1` : MadMapper lit lui-même parfois sur
l'adresse locale ; essayer l'adresse IP réseau du Mac
(`--ponk 192.168.x.y:5583`) ou le multicast (`--ponk 239.255.10.24:5583`).

## 3. Réglages conseillés côté MadMapper

Laser Studio envoie déjà des tracés préparés. Pour chaque chemin, il
demande à MadMapper :

| Métadonnée PONK | Valeur | Effet |
|---|---|---|
| `PRESRVOR` | 1 | garder l'ordre des chemins du studio |
| `PATHNUMB` | 0, 1, 2… | numéro du chemin dans la trame |

Le reste vient des réglages de la **surface laser** dans MadMapper :

- **Optimisation des angles** (`ANGLEOPT`) : laisser activée si les angles
  vifs s'arrondissent ; on peut la couper si le rendu est déjà bon.
- **Points minimum** (`MINIPNTS`) : laisser la valeur par défaut.
- **Vitesse de balayage** (`MAXSPEED`) : la valeur normale (1.0) est sûre ;
  ne pas l'augmenter sans connaître les limites des scanners.

Les déplacements éteints entre deux formes ne sont pas envoyés : MadMapper
les recalcule lui-même.

## 4. Sécurité

- **Deux barrières** : Laser Studio doit être armé, **et** la sortie laser
  de MadLaser activée. Désarmer l'un ou l'autre éteint le laser.
- **Échap** dans Laser Studio : arrêt d'urgence immédiat, trames vides
  envoyées tout de suite.
- Fermer Laser Studio (Ctrl+C) envoie d'abord des trames vides. Si le studio
  plante ou que le réseau coupe, MadMapper peut **garder la dernière
  image** : garder la main sur la sortie laser de MadMapper (et l'arrêt
  d'urgence du laser) pendant un show.
- Les tests automatiques n'envoient jamais de PONK vers MadMapper : ils
  utilisent un récepteur local sur un port libre, jamais le port 5583.
