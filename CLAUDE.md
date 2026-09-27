# ilda-laser — Laser Studio

## Goal

Build our own professional laser show software for macOS, **Laser Studio**
(`studio/` crate), inspired by Laserworld Showcontroller (laser-specific:
timeline, live effects, multi-projector), Pangolin QuickShow/Beyond (the pro
reference: huge effect library, cues, timecode, audience safety zones) and
MadMapper/MadLaser (modern UI, mapping, audio/MIDI/OSC reactivity).

Target hardware: a Laserworld **ShowNET** interface (192.168.129.51 on the
LAN) driving the user's lasers. `pi-receiver/` (Raspberry Pi DAC + HAT) is
**abandoned**: the ShowNET replaced it. Don't extend it or propose HAT/PCB
work. `pc-client/` is the original CLI; new work goes into `studio/`.

## Hard rules (never break these)

### Intellectual property
- **Never decompile, disassemble or extract secrets** from Showcontroller,
  Showeditor, Pangolin, MadMapper, TouchDesigner or the ShowNET firmware.
  In particular, never try to recover encryption keys for the ShowNET
  streaming protocol. It is encrypted; ShowNET output will come **only** from
  Laserworld's official API (requested under NDA). Until then the studio
  runs in preview mode, or on IDN / Ether Dream through `laser-dac`.
- **Never copy content** (effects, cues, shows, frames, fonts, icons) from
  Pangolin or Laserworld software or their bundled libraries into this
  repo. Studying their **public** docs, manuals, videos and feature lists
  for inspiration is fine; reimplement ideas in our own code.
- Content we ship must be **our own** (procedurally generated presets) or
  come from a source whose licence explicitly allows redistribution. Every
  third-party file added must be listed in `docs/CONTENT_SOURCES.md` with
  its URL, licence and date checked. No licence found = don't add it.
- The user's own ILDA files (`.ild`, bought or exported from software they
  own) are loaded at runtime through ILDA import. They are never committed.

### Laser safety
- The laser **always starts disarmed**. Only an explicit user action arms
  it (button / Space). Escape is always an instant blackout.
- Automated tests, e2e tests and agents **never** run the studio with
  `--device`, never arm a real laser, never talk to 192.168.129.51.
  All testing is preview-only.
- Keep the existing safety features working: brightness scaling,
  calibration clamping to -1..1, blanked travel between shapes. Any new
  output path must honour arm/disarm and blackout.

## Build & test

Rust is installed via Homebrew rustup; prefix commands with
`export PATH=/opt/homebrew/opt/rustup/bin:$PATH`.

```sh
cargo build -p laser-studio
cargo test -p laser-studio
cargo clippy -p laser-studio --all-targets -- -D warnings
cargo run -p laser-studio -- --port 8090 --data-dir /tmp/studio-test   # preview only
```

End-to-end UI tests (Playwright, headless Chromium) live in `studio/e2e/`:
`npm --prefix studio/e2e test`. They start their own studio instance on a
free port with a temp `--data-dir`, click through the UI and assert on
`/api/state` and `/api/frame`. Never point them at the user's running
instance (port 8080) or the user's `studio-data/`.

## Architecture (studio)

- `engine.rs`: `Settings` (a plain, serializable "look") + `Animator`
  (time-based state) → one frame of normalized points (x,y in -1..1,
  r,g,b in 0..1), then `densify` (max step, corner dwell).
- `patterns.rs`, `font.rs`: content generators. Closed outlines repeat
  their first point.
- `output.rs`: `Output` trait (arm/disarm/send). `DacOutput` = laser-dac.
  ShowNET will be another `Output` once the API arrives.
- `scenes.rs`: saved looks + playlist. `web.rs`: JSON HTTP API (localhost
  only). `index.html`: the UI (single file, no build step, **French** UI
  text). `main.rs`: CLI + 60 fps engine thread.
- Keep `Settings` backwards compatible (`#[serde(default)]`): saved scenes
  must keep loading.

## Team workflow (multi-agent)

Roles:
1. **Research agents** (read-only, web): study public information about
   the reference products and free content sources. Output a report in
   `docs/research/<topic>.md` with sources linked.
2. **Architect** (the main session): turns research into
   `docs/ROADMAP.md` — small, independent, testable work items, each with
   acceptance criteria — and assigns them.
3. **Developer agents**: one work item each, in an isolated git worktree,
   on a branch `feat/<short-name>` created from `develop`. Implement, add
   unit tests, run build + test + clippy, commit. The "pull request" is the
   branch plus a PR note file `docs/prs/<short-name>.md` (what/why, how it
   was tested, risks). No GitHub remote: PRs are local branches.
4. **Reviewer agents**: review one branch against `develop` (correctness,
   safety rules, IP rules, style, tests) in their own worktree, and append
   a "Review" section to the PR note ending in `Verdict: APPROVED` or
   `Verdict: CHANGES REQUESTED` (with a numbered list of required fixes).
   Reviews can run in parallel. Merges are serialised: one integrator (the
   architect, or a single merge agent) merges approved branches into
   `develop` one at a time with `git merge --no-ff feat/<name>`, re-runs
   the full suite on the merged result, and reverts the merge if it goes
   red. Never merge red builds.
5. **QA agents**: after merges, run the full suite plus e2e click tests on
   `develop`, write findings to `docs/qa/<date>.md`, file bugs as new
   roadmap items.

Branches: `main` = what the user has validated (only the user, or the
architect on the user's request, merges `develop` into `main`).
`develop` = integration branch. Work items never commit directly to
`develop` except via reviewed merges.

Conventions:
- Match the surrounding code: comment density, naming, error handling
  (`anyhow`), tests next to the code in `#[cfg(test)] mod tests`.
- Small PRs: one feature per branch. Don't reformat unrelated code.
- Commit messages: imperative summary line, body explaining why, ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Definition of done: builds, `cargo test` green, clippy clean with
  `-D warnings`, e2e green if UI changed, PR note written, safety and IP
  rules respected.
