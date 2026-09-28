# PR: import SVG and vectorise pictures into figures (T-297)

Branch: `feat/figure-import` · Task: `tasks/T-297-cues.md` · Depends on T-296 (figures)

## What / why

The user wants to turn their own logo or drawing into laser content quickly.
CRÉATION › Figures gets an **« Importer… »** action: drop (or pick) an SVG,
PNG or JPEG, adjust, see the preview, then **« Créer la figure »**. The
result is an ordinary T-296 `Figure`: saved in the library
(`studio-data/figures`), opened in the editor (editable, undoable), and
playable as a cue of the « Figures » page. The imported file itself is
never stored anywhere: the preview request carries it, only the figure the
user creates is saved.

### Rust: `studio/src/figure_import/` (new)

- `mod.rs`: options, sniffing by content (PNG / JPEG magic, `<svg`; gzip
  → « .svgz non pris en charge »; anything else → « format non reconnu »),
  default name from the file stem (made a valid figure name), and the
  common finish: fit into ±size keeping the aspect ratio (y flipped),
  Ramer-Douglas-Peucker simplification (iterative; closed paths split at
  their farthest point), **drawing order** by nearest neighbour from the
  top-left (open strokes may be reversed, closed ones start at their
  nearest vertex), and the **point budget** measured exactly as the engine
  will draw it (`Figure::frame_points` → `engine::densify` + travel back).
  Over budget: a little more simplification if that actually lowers the
  count (heavy simplification adds held corners, so it is not pushed),
  then the shortest strokes are left out, with a French warning; if a
  single stroke is still too heavy the figure is kept and flagged
  (« figure trop lourde … »). Also enforces T-296's limits (1 000 strokes,
  20 000 points). Stats: strokes, points, laser points, budget, travel
  before / after ordering, dropped, threshold used.
- `svg.rs`: our own reader on `roxmltree`. `path` (all commands M L H V C
  S Q T A Z, absolute/relative, implicit repeats, compact numbers and arc
  flags), `line`, `polyline`, `polygon`, `rect` (incl. rounded corners),
  `circle`, `ellipse`, `g`/`a`/`switch`/nested `svg`, `use` of the same
  file (`href` / `xlink:href`, depth 8, 5 000 uses max; external refs
  ignored with a warning). Transforms: `matrix translate scale rotate(a
  [cx cy]) skewX skewY`, composed through groups. Colours: stroke, else
  (option « Contour des formes pleines ») fill; `#rgb`, `#rrggbb`, `rgb()`
  with numbers or %, `currentColor`, 22 basic names, `url(#gradient)` →
  its fallback colour or the default colour; black / near-black → the
  default colour (invisible on a laser). Presentation attributes,
  `<style>` rules with a single class / id / tag / `*` selector, and the
  `style` attribute, in that precedence. `display:none`, `visibility`,
  `opacity:0` hidden. `text` and `image` are skipped with a warning
  (« convertissez le texte en contours »). Arcs → cubic Béziers (SVG
  F.6); curves flattened with a tolerance of 0.05 % of the drawing's
  diagonal (unit independent).
- `raster.rs`: decoding by `image` (PNG + JPEG only), then our own
  vectorisation on a working copy (area average, longest side 64–800 px,
  transparency composited on white). Modes: **Contours** (threshold, Otsu
  when automatic, border majority = background, « Inverser »; marching
  squares outlines incl. holes), **Lignes** (same mask, Zhang-Suen
  thinning, skeleton traced end/junction to end/junction, loops closed),
  **Bords** (Sobel on a blurred copy, threshold, thinning), **Couleurs**
  (deterministic k-means with N + 1 clusters, the border's cluster is the
  background, each other colour outlined in its colour boosted to full
  laser brightness). Specks and small holes under « Taches ignorées »² px
  removed, moving-average smoothing (« Lissage »), short lines dropped.

### Hostile files

- Body capped at 20 MB (read with `take`, 413 beyond), SVG at 5 MB,
  pictures at 8192 × 8192 and 192 MB of decoder memory (`image::Limits`;
  a header claiming 100 000² px is refused before allocating).
- XML: `<!ENTITY` refused outright (roxmltree has billion-laughs guards;
  this keeps even small ones out), no entity resolver, so nothing is ever
  fetched; roxmltree node limit 200 000. **roxmltree parses recursively**
  (a debug build overflowed a 2 MB stack at ~150 nested elements), so the
  raw text's nesting is pre-scanned (comments, CDATA, doctype and quoted
  attributes skipped) and anything over 96 levels is refused before
  parsing; our own walk stops at 64 with a warning.
- Every number must be finite (lexer), transforms and arc parameters
  checked; bad path data is kept up to the error (like browsers) with a
  warning; 200 000 segments and 500 000 flattened points max.
- The web handler runs the import on its own thread (64 MB stack) inside
  `catch_unwind`: a panic or overflow can't take the HTTP worker down;
  the Shared lock is not held while importing. All errors are French
  messages shown in red in the dialog.

### API (`web.rs`)

`POST /api/figures/import?name=&simplify=&size=&budget=&color=&fills=&mode=&threshold=&invert=&smooth=&colors=&resolution=&min_size=`
with the raw file as body → `{figure, warnings, stats}` (400 + French text
on error, 413 when too big). Preview only: nothing is saved (« Créer la
figure » uses the existing `POST /api/figures`). The budget defaults to
the mixer's point budget. Uses develop's `query_str` (T-232 brought one).

### UI (`index.html`, French)

« Importer… » next to Nouvelle / Enregistrer / Jouer opens a modal
`<dialog>`: the **rights reminder** (« N'importez que des fichiers dont
vous avez les droits … Le fichier n'est pas conservé »), a drop zone +
file picker, a preview canvas (drawing order with the dashed blanked
travel, same renderer as the editor), stats (strokes, points, laser points
vs budget, travel before → after ordering, threshold), warnings / errors,
and settings: Simplification, Taille, Budget de points (defaults to the
current budget), Couleur; SVG: Contour des formes pleines; picture: Mode,
Seuil + automatique, Inverser, Lissage, Nombre de couleurs, Résolution,
Taches ignorées (only the active mode's settings are enabled). Changes
re-preview after 250 ms; « Créer la figure » waits for a pending preview
so it creates what the settings show, asks before replacing an existing
figure name, then opens the figure in the editor (undo returns to the
previous one). Editor shortcuts are off while the dialog is open; Échap
keeps its blackout (the global handler runs first).

## Licences (new dependencies, checked 2026-09-28 with `cargo metadata`)

| Crate | Version | Licence |
|---|---|---|
| roxmltree | 0.21.1 | MIT OR Apache-2.0 |
| image (features `png`, `jpeg` only) | 0.25.10 | MIT OR Apache-2.0 |
| png | 0.18.1 | MIT OR Apache-2.0 |
| zune-jpeg / zune-core | 0.5.15 / 0.5.3 | MIT OR Apache-2.0 OR Zlib |
| fdeflate, flate2, crc32fast, bitflags, cfg-if, num-traits | – | MIT OR Apache-2.0 |
| miniz_oxide | 0.8.9 / 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| byteorder-lite | 0.1.0 | Unlicense OR MIT |
| moxcms, pxfm (colour management pulled by `image`) | 0.8.1 / 0.1.30 | BSD-3-Clause OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |

Every crate is usable under MIT or Apache-2.0. No file is added to the
repo (no fonts, images or SVGs): tests generate their own SVGs and PNG /
JPEG pictures (Rust via `image`'s encoders, e2e via a 30-line PNG encoder
on `node:zlib`), so `docs/CONTENT_SOURCES.md` is unchanged.

## Testing

- `cargo test -p laser-studio`: **576 passed** (+ 2 shutdown tests),
  after rebasing on develop ad2e974. New: 19 tests —
  `figure_import::tests` (sniffing, default names, RDP incl. closed
  paths, nearest-neighbour order cuts the travel > 50 % and reverses open
  strokes, fit + closed outlines + budget measured through `densify`,
  over-budget flag, non-finite input), `svg::tests` (every shape with
  colours and exact geometry, every path command incl. S/T reflection and
  arcs, nested transforms incl. rotate-about-point and skew, `<style>`
  classes / ids / tags, `use` with xlink, gradients, hidden things, text /
  image / external warnings, fills off; hostile: entity bomb, external
  entity, truncated XML, non-SVG root, bad numbers (1e999, NaN), truncated
  path data, 100 000-deep nesting refused, depth cut, `use` loop bounded,
  too many segments; paint / transform parsing), `raster::tests` (disc +
  square outlines on the radius, light-on-dark auto background and
  Inverser, centre lines of a cross, edges, colours separated and boosted
  with a transparent area, JPEG with specks removed, truncated PNG / JPEG,
  100 000² header, blank and 1 × 1 pictures in every mode), and
  `web::tests::figure_import_previews_without_saving_and_refuses_bad_files`
  (query parsing, percent-decoded name, nothing saved, 400s, deep nesting
  through the HTTP worker, server still up).
- `cargo clippy -p laser-studio --all-targets -- -D warnings`: clean.
- e2e: new `studio/e2e/tests/figure-import.spec.ts` (3 tests): an SVG logo
  (square, scaled circle in a group, Bézier, black polygon) previews,
  re-previews on a size change, is created, saved identical in colours /
  shape / size, editable (mirror + save), plays as a cue with a non-empty
  frame, laser still off (`armed` false, `output_lit` 0); a generated PNG
  becomes 4 outlines under a 600-point budget, a 150-point budget drops
  details with a warning and stays under, Lignes mode works, the created
  figure is in the library; colours mode separates red and blue; 9 bad
  files (text, empty, broken SVG, entity bomb, external entity, text-only
  SVG, 20 000-deep nesting, truncated PNG, blank PNG) each show a red
  French message with « Créer » disabled, invalid name refused, a 21 MB
  body gets 413, the studio still answers. Screenshots
  `test-results/figure-import-dialog.png`, `test-results/figure-import.png`.
- Full e2e suite after the rebase: **148 passed**, 0 failed.
- Timing (debug build, pathological 1600² noise picture): 0.5–1.3 s at
  the default 320 px working size, up to ~7 s in Couleurs mode at 800 px.

## Risks

- **The HTTP worker is busy during an import** (one worker thread for all
  non-stop requests): a heavy picture at the maximum resolution can pause
  `/api/state` / `/api/frame` polling for a few seconds in a debug build
  (far less in release). `POST /api/estop` is handled on the receiving
  thread and is not affected; the engine and the arm gate are untouched.
  Reviewer: acceptable, or cap the resolution lower / move imports to a
  separate queue?
- Nesting over 96 levels is refused (real exports are far shallower);
  `<!ENTITY` declarations are refused even when harmless.
- Not supported (by design, warned or silently skipped): text (convert to
  outlines), embedded images, clip paths / masks (the clipped shapes are
  drawn whole), percentage lengths, nested `svg` viewBox scaling,
  `stroke-width` (a laser line has no width), CSS beyond single simple
  selectors, gradients (drawn in the default colour).
- The budget is per image and measured with the current densify
  constants; if `engine::densify` changes, the figure is re-measured on
  the next import only (the mixer still enforces the live budget).
- Colours mode draws shared borders between two colour regions twice
  (once per colour), like most outline tracers.
- New dependencies grow the build (`image` with PNG + JPEG only).

## Review
