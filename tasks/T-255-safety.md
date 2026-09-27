---
id: T-255
title: Mode balayage public verrouillé par défaut
status: todo
area: safety
priority: P1
depends_on: [T-003, T-250, T-252, T-254, T-256, T-258]
owner: ""
branch: ""
source: docs/research/safety-regulation.md#22-mpe-for-audience-exposure
---

## Contexte
Un faisceau de show de quelques watts dépasse l'exposition maximale permise (≈ 2,5 mW/cm² pour un faisceau fixe, 10 mW/cm² en balayage continu selon l'ILDA) par un facteur de centaines à milliers. Le balayage du public n'est possible qu'avec atténuation, divergence et un système anti-défaut de balayage **matériel**. Par défaut, Laser Studio doit donc garder les faisceaux au-dessus du public, et rendre le mode public explicite, difficile à activer et temporaire.

## À faire
- Deux modes de sortie : **« Au-dessus du public »** (défaut) et **« Balayage public »**.
- Mode « Au-dessus du public » : verrou `audience_zone` → l'armement est refusé tant qu'aucune zone Blank « Public » ou horizon (T-003) n'est définie pour la sortie. Les zones Dim ne suffisent pas dans ce mode.
- Déverrouiller « Balayage public » exige, dans cet ordre :
  1. au moins une zone Dim marquée « Public » avec une atténuation ≥ 90 % (réglable jusqu'à 50 % minimum en connaissance de cause) ;
  2. fiche projecteur (T-254) complète, avec `hw_scan_fail == Some(true)` déclaré ;
  3. liste de contrôle (T-258) terminée avec les items « mesure d'irradiance faite au point le plus proche du public » et « lentille de divergence / atténuation matérielle » cochés ;
  4. mode maintien (T-252) activé ;
  5. garde anti-point fixe (T-256) en profil strict ;
  6. saisie de la phrase « JE PRENDS LA RESPONSABILITÉ » dans une boîte de confirmation.
- Le mode public **expire** à la fin de la session (redémarrage, changement de profil, arrêt d'urgence) et n'est jamais restauré au démarrage.
- Le déverrouillage désarme d'abord le laser ; il faut réarmer ensuite.

## Modèle de données
```rust
pub enum AudienceMode { AboveOnly, Scanning { unlocked_at: SystemTime } }
// Zone de T-003 : ajouter `audience: bool` (#[serde(default)])
```
`AudienceMode` vit dans l'état de session, **pas** dans un fichier de réglages.

## Interface
- Section Sécurité : « Mode de sortie : Au-dessus du public / Balayage public ». Le second ouvre un assistant pas-à-pas qui montre chaque condition (coche verte / croix rouge) avec un lien vers le réglage.
- En mode public : bordure orange permanente autour de l'aperçu et libellé « BALAYAGE PUBLIC » dans la barre d'état.

## Critères d'acceptation
- [ ] Installation neuve : armement refusé tant qu'aucune zone Public/horizon n'existe (message clair)
- [ ] Chaque condition manquante bloque l'assistant avec son libellé
- [ ] Redémarrage → mode « Au-dessus du public »
- [ ] Arrêt d'urgence → mode « Au-dessus du public »
- [ ] Journal (T-259) : déverrouillage enregistré avec la liste des conditions

## Tests
Unitaires sur l'état et les conditions ; e2e de l'assistant complet en aperçu (déclarations fictives), vérifier l'expiration après reset d'arrêt d'urgence.

## Notes
- Le logiciel ne rend pas le balayage du public sûr : il oblige seulement à passer par les étapes. Le texte de l'assistant doit le dire (« Le logiciel ne détecte pas une panne des galvos »).
- Belgique : pas de permis spécifique trouvé pour le balayage du public, mais la responsabilité civile reste entière (voir rapport §3.2). France (arrêté du 18 juin 2026, à vérifier) : pas de tir 3B/4 vers le public.
- CLAUDE.md : tests jamais avec `--device`.

## Journal
- 2026-09-27 — agent de recherche : tâche créée depuis `docs/research/safety-regulation.md`.
