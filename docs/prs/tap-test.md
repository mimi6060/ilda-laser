# fix/tap-test — Tap tempo e2e robuste à la charge (T-350)

## Quoi / pourquoi
Le test tapait 4 fois à 500 ms et exigeait > 110 BPM ; sous charge chaque clic prend plus longtemps (109 BPM mesurés, à raison). Le test note maintenant l’instant réel de chaque tap dans la page et compare le BPM du studio à ces taps (±8 %).

`scripts/integrate.sh` relance une fois, seuls, les tests e2e en échec avant de déclarer la suite rouge.

## Tests
`tempo.spec.ts:62` ×3 vert sous charge (load ~15).

## Risques
Aucun code du studio touché.

## Review
