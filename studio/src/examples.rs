//! Built-in example shows (T-124 to T-129): six ready-made timelines of
//! our own, written here in code from our own catalogue cues
//! (`presets.rs`). They are read-only: opening one loads a copy in the
//! player (nothing is written to `studio-data/shows`), and *Enregistrer*
//! saves that copy under a name of the operator's choosing.
//!
//! Each one is an arrangement on a bar grid in 4/4, inspired only by the
//! public descriptions of festival looks (docs/research/festival-looks.md,
//! section 6); no cue, show or content of another product is used. They
//! are *Temps* shows: positions are beats of the live tempo clock, so they
//! start on the next bar and follow the BPM (at 140 instead of 128 BPM the
//! whole show is shorter in proportion). The tempo map holds the tempo the
//! arrangement was written for, shown as the « tempo conseillé ».
//!
//! They play through the normal path like any show: layers, point budget,
//! live modifiers, calibration, the whole safety stage and the arm gate.
//! Nothing here can arm the laser.

use crate::engine::{Content, Settings};
use crate::live::{ColorOverride, Rate};
use crate::presets::Preset;
use crate::timeline::{Dur, Event, EventSource, Marker, Show, TempoPoint, TimeBase, Track};
use anyhow::{anyhow, Result};
use serde::Serialize;

/// Beats per bar of every example.
const BPB: f64 = 4.0;

const WHITE: [u8; 3] = [255, 255, 255];
const WARM_WHITE: [u8; 3] = [255, 236, 200];
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const CYAN: [u8; 3] = [0, 255, 255];
const VIOLET: [u8; 3] = [140, 60, 255];
const MAGENTA: [u8; 3] = [255, 0, 255];
const AMBER: [u8; 3] = [255, 140, 0];
const GOLD: [u8; 3] = [255, 190, 40];
/// The « couleur de marque » of the DJ entrance: the studio's own cyan.
const BRAND: [u8; 3] = [34, 227, 255];

/// Marker colours: section, build-up, drop / black.
const M_SECTION: [u8; 3] = [77, 163, 255];
const M_BUILD: [u8; 3] = [255, 176, 32];
const M_DROP: [u8; 3] = [255, 77, 79];

/// An example as listed by `GET /api/timeline/examples`.
#[derive(Clone, Debug, Serialize)]
pub struct ExampleInfo {
    pub id: &'static str,
    /// Display title (may hold arrows); the loaded show is called `name`.
    pub title: &'static str,
    pub name: &'static str,
    pub bpm: f32,
    pub bars: u32,
    pub description: &'static str,
}

/// The six examples, in menu order. Show names are valid file names, so
/// the copy can be saved as it is.
pub const EXAMPLES: &[ExampleInfo] = &[
    ExampleInfo {
        id: "montee-drop",
        title: "Montée 16 mesures → drop",
        name: "Montée 16 mesures vers le drop",
        bpm: 128.0,
        bars: 32,
        description: "Chaser qui accélère, tunnel et strobe, une mesure de faisceau chaud, un temps de noir, puis le drop en blanc.",
    },
    ExampleInfo {
        id: "bloc-techno",
        title: "Bloc techno",
        name: "Bloc techno",
        bpm: 130.0,
        bars: 64,
        description: "Grille verte, coups sur le kick, break rouge au faisceau chaud, lignes rouge et blanc, tunnel de faisceaux qui change de sens.",
    },
    ExampleInfo {
        id: "break-trance",
        title: "Break trance → drop euphorique",
        name: "Break trance et drop euphorique",
        bpm: 138.0,
        bars: 48,
        description: "Nappes cyan puis bleues, aurore violette très lente, tunnel qui se ferme vers le blanc, drop cyan et blanc, final arc-en-ciel.",
    },
    ExampleInfo {
        id: "attaque-hardstyle",
        title: "Attaque hardstyle",
        name: "Attaque hardstyle",
        bpm: 150.0,
        bars: 32,
        description: "Coups rouges très courts, contretemps rouge / blanc, montée au chaser, deux temps de noir, drop blanc et rouge sur chaque temps.",
    },
    ExampleInfo {
        id: "entree-dj",
        title: "Entrée du DJ",
        name: "Entrée du DJ",
        bpm: 126.0,
        bars: 32,
        description: "Ciel étoilé dans le noir, rideaux couleur de marque, nom de l'artiste, compte à rebours 4-3-2-1, soleil ambre puis blanc.",
    },
    ExampleInfo {
        id: "hymne-cloture",
        title: "Hymne de clôture",
        name: "Hymne de clôture",
        bpm: 124.0,
        bars: 64,
        description: "Soleil levant ambre, nappe dorée qui respire, vague ambre et magenta, montée au blanc, grand éventail, fondu au noir.",
    },
];

/// Beat where bar `n` (1-based) starts.
fn bar(n: f64) -> f64 {
    (n - 1.0) * BPB
}

/// Builds one example, resolving cue names against the catalogue.
struct Builder<'a> {
    presets: &'a [Preset],
    show: Show,
    missing: Vec<String>,
}

/// An event under construction: `.color()`, `.dim()`… then `Builder::put`.
struct Ev(Event);

impl Ev {
    fn color(mut self, rgb: [u8; 3]) -> Self {
        self.0.modifiers.color = ColorOverride::Fixed { rgb };
        self
    }
    fn dim(mut self, brightness: f32) -> Self {
        self.0.modifiers.brightness = brightness;
        self
    }
    fn size(mut self, size: f32) -> Self {
        self.0.modifiers.size = size;
        self
    }
    fn width(mut self, size_x: f32) -> Self {
        self.0.modifiers.size_x = size_x;
        self
    }
    /// Animation speed of the content (0..4).
    fn speed(mut self, speed: f32) -> Self {
        self.0.modifiers.speed = speed;
        self
    }
    /// Turns per bar around the beam axis (negative: the other way).
    fn spin(mut self, turns_per_bar: f32) -> Self {
        self.0.modifiers.rot_speed = [0.0, 0.0, turns_per_bar];
        self.0.modifiers.rot_sync = true;
        self
    }
    fn rainbow(mut self, bars: f64) -> Self {
        self.0.modifiers.color = ColorOverride::Rainbow { spread: 1.0, rate: Rate::Beats((bars * BPB) as f32) };
        self
    }
    fn fade_in(mut self, beats: f64) -> Self {
        self.0.fade_in = Dur::Beats(beats);
        self
    }
    fn fade_out(mut self, beats: f64) -> Self {
        self.0.fade_out = Dur::Beats(beats);
        self
    }
    /// The content starts this many of its own beats in.
    fn offset(mut self, beats: f64) -> Self {
        self.0.offset_beats = beats;
        self
    }
}

impl<'a> Builder<'a> {
    fn new(info: &ExampleInfo, presets: &'a [Preset], tracks: &[&str]) -> Self {
        let tracks = tracks
            .iter()
            .enumerate()
            .map(|(i, name)| Track { name: name.to_string(), layer: (i % 4) as u8 + 1, ..Default::default() })
            .collect();
        let show = Show {
            name: info.name.to_string(),
            time_base: TimeBase::Beats,
            // Temps shows play on the live tempo; this is the tempo the
            // arrangement was written for (« tempo conseillé »).
            tempo_map: vec![TempoPoint { at_s: 0.0, bpm: info.bpm, beats_per_bar: BPB as u8 }],
            tracks,
            ..Default::default()
        };
        Self { presets, show, missing: Vec::new() }
    }

    /// The built-in cue called `name` (never a figure, ILDA file or
    /// operator cue that happens to have the same name).
    fn find(&mut self, name: &str) -> Option<&'a Preset> {
        let own = [crate::figures::CATEGORY, crate::ilda_library::CATEGORY, crate::usercues::CATEGORY];
        let found = self.presets.iter().find(|p| p.name == name && !own.contains(&p.category));
        if found.is_none() {
            self.missing.push(name.to_string());
        }
        found
    }

    /// A catalogue cue from bar `from` (1-based, may be fractional) for `beats` beats.
    fn cue(&mut self, name: &str, from: f64, beats: f64) -> Ev {
        let id = self.find(name).map_or_else(|| name.to_string(), |p| p.id.clone());
        Ev(Event { start: bar(from), len: beats, source: EventSource::Cue { id }, ..Default::default() })
    }

    /// A look stored in the show: a catalogue cue's settings with a tweak.
    fn look(&mut self, name: &str, from: f64, beats: f64, tweak: impl FnOnce(&mut Settings)) -> Ev {
        let mut settings = self.find(name).map(|p| p.settings.clone()).unwrap_or_default();
        tweak(&mut settings);
        Ev(Event { start: bar(from), len: beats, source: EventSource::Look { settings }, ..Default::default() })
    }

    fn put(&mut self, track: usize, ev: Ev) {
        self.show.tracks[track].events.push(ev.0);
    }

    fn marker(&mut self, at_bar: f64, name: &str, color: [u8; 3]) {
        self.show.markers.push(Marker { at: bar(at_bar), name: name.to_string(), color });
    }

    fn finish(mut self) -> Result<Show> {
        if !self.missing.is_empty() {
            return Err(anyhow!("cue introuvable dans le catalogue : {}", self.missing.join(", ")));
        }
        self.show.sanitize();
        Ok(self.show)
    }
}

/// The example `id`, built from `presets` (the studio's cue list).
pub fn build(id: &str, presets: &[Preset]) -> Result<Show> {
    let info = EXAMPLES.iter().find(|e| e.id == id).ok_or_else(|| anyhow!("exemple inconnu : {}", id.chars().take(40).collect::<String>()))?;
    match info.id {
        "montee-drop" => montee_drop(info, presets),
        "bloc-techno" => bloc_techno(info, presets),
        "break-trance" => break_trance(info, presets),
        "attaque-hardstyle" => attaque_hardstyle(info, presets),
        "entree-dj" => entree_dj(info, presets),
        _ => hymne_cloture(info, presets),
    }
}

/// Generator parameters of a look, for the tweaks below.
fn params(s: &mut Settings) -> Option<&mut crate::generators::GenParams> {
    match &mut s.content {
        Content::Generator { params, .. } => Some(params),
        _ => None,
    }
}

/// T-124: 16 bars of build-up, then the drop (32 bars).
fn montee_drop(info: &ExampleInfo, presets: &[Preset]) -> Result<Show> {
    let mut b = Builder::new(info, presets, &["Faisceaux", "Accents", "Strobe", "Nappe"]);
    // 1-4: a chaser stepping on every beat, low and narrow.
    let e = b.cue("Chaser centre → bords · cyan", 1.0, 16.0).size(0.8).dim(0.8);
    b.put(0, e);
    // 5-8: the same chase at 1/2 beat, a sheet rising under it.
    let e = b.cue("Chaser → · vert", 5.0, 16.0).size(0.85);
    b.put(0, e);
    let e = b.cue("Nappe qui monte · une fois", 5.0, 16.0).dim(0.5);
    b.put(3, e);
    // 9-12: the fan fills up towards white, beams opening in the back.
    let e = b.cue("Chaser remplissage · bleu", 9.0, 16.0).color(WHITE).size(0.9);
    b.put(0, e);
    let e = b.cue("Faisceaux qui s'ouvrent lents · cyan", 9.0, 16.0).dim(0.6).fade_in(8.0);
    b.put(1, e);
    // 13-15: a tunnel spinning; 1/4-beat strobe from bar 14.
    let e = b.cue("Tunnel 4 anneaux · cyan", 13.0, 12.0).color(WHITE).spin(0.5);
    b.put(0, e);
    let e = b.look("Strobe 1/2 temps · blanc", 14.0, 8.0, |s| s.strobe_div = 4.0).dim(0.6);
    b.put(2, e);
    // 16: hot white beam on beats 1-3, beat 4 dark.
    let e = b.cue("Faisceau chaud · blanc", 16.0, 3.0);
    b.put(0, e);
    // 17: the drop - wide white fan at full level, 1/2-beat strobe for one bar.
    let e = b.cue("Éventail 12 · vert", 17.0, 4.0).color(WHITE).size(1.2);
    b.put(0, e);
    let e = b.cue("Strobe 1/2 temps · blanc", 17.0, 4.0);
    b.put(2, e);
    // 18-24: crossing scissors with their white flash, stabs behind.
    let e = b.cue("Ciseaux (X) flash blanc · vert / jaune", 18.0, 28.0);
    b.put(0, e);
    let e = b.cue("Coups sur le kick · blanc", 18.0, 28.0).dim(0.5);
    b.put(1, e);
    // 25-32: rotating rays, new colour at 25.
    let e = b.cue("Soleil 12 rayons · jaune", 25.0, 32.0).color(MAGENTA).spin(0.25);
    b.put(0, e);
    let e = b.cue("Chaser aller-retour · blanc / bleu", 25.0, 32.0).dim(0.6);
    b.put(1, e);
    for (at, name, color) in [
        (1.0, "Intro", M_SECTION),
        (5.0, "Montée", M_BUILD),
        (9.0, "Ouverture vers le blanc", M_BUILD),
        (13.0, "Tunnel + strobe", M_BUILD),
        (16.0, "Faisceau chaud, noir", M_DROP),
        (17.0, "DROP", M_DROP),
        (18.0, "Ciseaux", M_SECTION),
        (25.0, "Rayons", M_SECTION),
    ] {
        b.marker(at, name, color);
    }
    b.finish()
}

/// T-125: a 64-bar techno block, to play in a loop.
fn bloc_techno(info: &ExampleInfo, presets: &[Preset]) -> Result<Show> {
    let mut b = Builder::new(info, presets, &["Faisceaux", "Accents", "Effets", "Fond"]);
    // 1-16: a tightening green grid, no strobe.
    let e = b.cue("Grille de balayage 5 · vert", 1.0, 64.0).fade_in(4.0);
    b.put(0, e);
    // 17-32: stabs on the kick, narrow fan, positions on every beat.
    let e = b.cue("Coups sur le kick · blanc", 17.0, 64.0).color(GREEN).width(0.5);
    b.put(0, e);
    let e = b.cue("Positions temps + contretemps · magenta", 17.0, 64.0).color(GREEN).dim(0.5);
    b.put(1, e);
    // 33-40: break, the hot beam panning over 16 beats, red.
    let e = b.cue("Faisceau chaud · rouge", 33.0, 32.0).fade_in(2.0);
    b.put(0, e);
    // 41-48: scanning lines, red and white.
    let e = b.cue("Lignes de balayage 5 · rouge / bleu", 41.0, 32.0).color(RED);
    b.put(0, e);
    let e = b.cue("Lignes de balayage 3 · vert", 41.0, 32.0).color(WHITE).dim(0.6).offset(2.0);
    b.put(1, e);
    // 49-64: a green beam tunnel turning 1/8 turn a beat, the other way
    // every 8 bars, a stab on every beat.
    let e = b.cue("Cône 10 · cyan", 49.0, 32.0).color(GREEN).spin(0.5);
    b.put(0, e);
    let e = b.cue("Cône 10 · cyan", 57.0, 32.0).color(GREEN).spin(-0.5);
    b.put(0, e);
    let e = b.cue("Coups sur le kick · blanc", 49.0, 64.0).color(GREEN).dim(0.4);
    b.put(1, e);
    b.show.loop_region = Some((0.0, bar(65.0)));
    for (at, name, color) in [
        (1.0, "Grille", M_SECTION),
        (17.0, "Coups sur le kick", M_SECTION),
        (33.0, "Break rouge", M_BUILD),
        (41.0, "Lignes rouge et blanc", M_BUILD),
        (49.0, "Tunnel de faisceaux", M_DROP),
        (57.0, "Sens inverse", M_SECTION),
    ] {
        b.marker(at, name, color);
    }
    b.finish()
}

/// T-126: a trance breakdown and its euphoric drop (48 bars).
fn break_trance(info: &ExampleInfo, presets: &[Preset]) -> Result<Show> {
    let mut b = Builder::new(info, presets, &["Faisceaux", "Accents", "Strobe", "Fond"]);
    // 1-16: a ceiling coming down twice, cyan then blue.
    let e = b.cue("Nappe (liquid sky) · bleu", 1.0, 32.0).color(CYAN).fade_in(8.0);
    b.put(0, e);
    let e = b.cue("Nappe (liquid sky) · bleu", 9.0, 32.0);
    b.put(0, e);
    // 17-24: a very slow violet-blue aurora.
    let e = b.cue("Nappe (liquid sky) · cyan → magenta", 17.0, 32.0).color(VIOLET).speed(0.3).fade_in(4.0);
    b.put(0, e);
    let e = b.cue("Ciel étoilé lent · bleu", 17.0, 32.0).dim(0.4);
    b.put(3, e);
    // 25-32: the tunnel closes towards white and spins faster; 1/2-beat
    // strobe in bar 31; black on the last beat of bar 32.
    let e = b.cue("Tunnel 8 anneaux · cyan", 25.0, 16.0).size(1.1).spin(0.125);
    b.put(0, e);
    let e = b.cue("Tunnel 8 anneaux · cyan", 29.0, 15.0).color(WHITE).size(0.6).spin(1.0);
    b.put(0, e);
    let e = b.cue("Strobe 1/2 temps · blanc", 31.0, 4.0).dim(0.7);
    b.put(2, e);
    // 33-40: the drop, cyan and white.
    let e = b.cue("Cône 16 · cyan", 33.0, 32.0).spin(0.5);
    b.put(0, e);
    let e = b.cue("Coups sur le kick · blanc", 33.0, 32.0).dim(0.7);
    b.put(1, e);
    // 41-48: a slowly drifting rainbow beam tunnel, 1/4 turn a beat - the
    // only rainbow on purpose.
    let e = b.cue("Cône 16 · arc-en-ciel", 41.0, 32.0).spin(1.0).fade_out(4.0);
    b.put(0, e);
    for (at, name, color) in [
        (1.0, "Plafond cyan", M_SECTION),
        (9.0, "Plafond bleu", M_SECTION),
        (17.0, "Aurore", M_SECTION),
        (25.0, "Le tunnel se ferme", M_BUILD),
        (31.0, "Strobe", M_BUILD),
        (32.75, "Noir", M_DROP),
        (33.0, "DROP", M_DROP),
        (41.0, "Tunnel arc-en-ciel", M_SECTION),
    ] {
        b.marker(at, name, color);
    }
    b.finish()
}

/// T-127: a hardstyle attack at 150 BPM (32 bars).
fn attaque_hardstyle(info: &ExampleInfo, presets: &[Preset]) -> Result<Show> {
    let mut b = Builder::new(info, presets, &["Faisceaux", "Accents", "Effets", "Nappe"]);
    // 1-8: red stabs with a hard 0.15-beat gate.
    let e = b.look("Coups sur le kick · blanc", 1.0, 32.0, |s| {
        s.color = RED;
        s.gate_beats = 0.15;
    });
    b.put(0, e);
    // 9-16: positions on the off-beat too, red / white each bar.
    for i in 0..8 {
        let color = if i % 2 == 0 { RED } else { WHITE };
        let e = b.cue("Positions temps + contretemps · magenta", 9.0 + i as f64, 4.0).color(color);
        b.put(0, e);
    }
    // 17-24: the build, a chaser at 1/2 then 1/4 beat over a rising sheet;
    // black on the last two beats of bar 24.
    let e = b.cue("Chaser → · vert", 17.0, 16.0).color(RED);
    b.put(0, e);
    let e = b.look("Chaser → · vert", 21.0, 14.0, |s| {
        s.color = WHITE;
        if let Some(p) = params(s) {
            p.steps_per_beat = 4.0;
        }
    });
    b.put(0, e);
    let e = b.cue("Nappe qui monte · une fois", 17.0, 16.0).color(RED).dim(0.5);
    b.put(3, e);
    let e = b.cue("Nappe qui monte · une fois", 21.0, 14.0).color(WHITE).dim(0.6);
    b.put(3, e);
    // 25-32: the drop - a wide white fan stabbing every beat, red
    // positions moving on every beat.
    let e = b.look("Coups sur le kick · blanc", 25.0, 32.0, |s| {
        if let Some(p) = params(s) {
            p.count = 12;
        }
    });
    b.put(0, e);
    let e = b.cue("Positions temps + contretemps · magenta", 25.0, 32.0).color(RED).dim(0.8);
    b.put(1, e);
    for (at, name, color) in [
        (1.0, "Coups rouges", M_SECTION),
        (9.0, "Contretemps", M_SECTION),
        (17.0, "Montée", M_BUILD),
        (24.5, "Noir", M_DROP),
        (25.0, "DROP", M_DROP),
    ] {
        b.marker(at, name, color);
    }
    b.finish()
}

/// T-128: the headliner's entrance (32 bars). The artist's name is a text
/// look: the operator types theirs in the cue, or drops their own figure.
fn entree_dj(info: &ExampleInfo, presets: &[Preset]) -> Result<Show> {
    let mut b = Builder::new(info, presets, &["Faisceaux", "Texte", "Effets", "Fond"]);
    // 1-8: black, a sparse white starry sky.
    let e = b.cue("Ciel étoilé lent · bleu", 1.0, 32.0).color(WHITE).dim(0.7).fade_in(8.0);
    b.put(3, e);
    // 9-16: a corridor of lines in the brand colour.
    let e = b.cue("Lignes de balayage 3 · vert", 9.0, 32.0).color(BRAND).fade_in(4.0);
    b.put(0, e);
    // 17-20: the artist's name, held.
    let e = b.cue("DJ · cyan", 17.0, 16.0).color(BRAND);
    b.put(1, e);
    // 21-24: countdown 4-3-2-1, one digit a bar (the countdown generator,
    // T-108, counting from the event's start); black on beat 4 of bar 24.
    let e = b.cue("Compte à rebours 4-3-2-1 · blanc", 21.0, 15.0);
    b.put(1, e);
    // 25-32: rotating rays, amber then white, full level.
    let e = b.cue("Soleil 24 rayons · jaune", 25.0, 16.0).color(AMBER).spin(0.25);
    b.put(0, e);
    let e = b.cue("Soleil 24 rayons · jaune", 29.0, 16.0).color(WHITE).spin(0.25);
    b.put(0, e);
    let e = b.cue("Coups sur le kick · blanc", 25.0, 32.0).dim(0.5);
    b.put(2, e);
    for (at, name, color) in [
        (1.0, "Ciel étoilé", M_SECTION),
        (9.0, "Couloir couleur de marque", M_SECTION),
        (17.0, "Nom de l'artiste", M_BUILD),
        (21.0, "Compte à rebours", M_BUILD),
        (24.75, "Noir", M_DROP),
        (25.0, "Entrée !", M_DROP),
    ] {
        b.marker(at, name, color);
    }
    b.finish()
}

/// T-129: a sunset anthem to close the night (64 bars).
fn hymne_cloture(info: &ExampleInfo, presets: &[Preset]) -> Result<Show> {
    let mut b = Builder::new(info, presets, &["Faisceaux", "Accents", "Effets", "Nappe"]);
    // 1-16: a rising sun, amber then warm white.
    let e = b.cue("Soleil 12 rayons · jaune", 1.0, 32.0).color(AMBER).spin(0.0625).fade_in(16.0);
    b.put(0, e);
    let e = b.cue("Soleil 12 rayons · jaune", 9.0, 32.0).color(WARM_WHITE).spin(0.0625);
    b.put(0, e);
    // 17-32: a gold sheet breathing slowly.
    let e = b.cue("Nappe (liquid sky) · arc-en-ciel", 17.0, 64.0).color(GOLD).speed(0.5).fade_in(4.0);
    b.put(3, e);
    // 33-40: a wave of beams, amber and magenta.
    let e = b.cue("Vague de faisceaux 12 · vert", 33.0, 32.0).color(AMBER);
    b.put(0, e);
    let e = b.cue("Vague de faisceaux 8 · vert", 33.0, 32.0).color(MAGENTA).dim(0.6).offset(2.0);
    b.put(1, e);
    // 41-48: beams opening up towards white.
    let e = b.cue("Faisceaux qui s'ouvrent lents · cyan", 41.0, 32.0).color(WHITE).fade_in(8.0);
    b.put(0, e);
    let e = b.cue("Nappe qui monte · une fois", 41.0, 16.0).color(WHITE).dim(0.5);
    b.put(3, e);
    // 49-60: a big white fan, slow; a slow rainbow over 57-60.
    let e = b.cue("Éventail 12 · vert", 49.0, 32.0).color(WHITE).size(1.2).speed(0.5);
    b.put(0, e);
    let e = b.cue("Éventail 12 · vert", 57.0, 16.0).rainbow(4.0).size(1.2).speed(0.5);
    b.put(0, e);
    // 61-64: one warm white beam, fading to black over 4 bars.
    let e = b.cue("Faisceau chaud · blanc", 61.0, 16.0).color(WARM_WHITE).fade_out(16.0);
    b.put(0, e);
    for (at, name, color) in [
        (1.0, "Soleil levant", M_SECTION),
        (17.0, "Nappe dorée", M_SECTION),
        (33.0, "Vague", M_SECTION),
        (41.0, "Montée au blanc", M_BUILD),
        (49.0, "Grand éventail", M_DROP),
        (57.0, "Arc-en-ciel", M_SECTION),
        (61.0, "Fondu au noir", M_DROP),
    ] {
        b.marker(at, name, color);
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{valid_show_name, Clock, Player};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn catalog() -> Vec<Preset> {
        crate::presets::catalog()
    }

    fn all(presets: &[Preset]) -> Vec<(&'static ExampleInfo, Show)> {
        EXAMPLES.iter().map(|e| (e, build(e.id, presets).unwrap_or_else(|err| panic!("{}: {err:#}", e.id)))).collect()
    }

    #[test]
    fn every_example_builds_from_catalogue_cues_only() {
        let presets = catalog();
        assert_eq!(EXAMPLES.len(), 6);
        for (info, show) in all(&presets) {
            assert!(valid_show_name(&show.name), "{}: « {} » must be a file name", info.id, show.name);
            assert_eq!(show.name, info.name);
            assert_eq!(show.time_base, TimeBase::Beats, "{} follows the live tempo", info.id);
            assert_eq!(show.tempo_map.len(), 1);
            assert_eq!(show.tempo_map[0].bpm, info.bpm);
            assert_eq!(show.end(), info.bars as f64 * BPB, "{}: {} bars", info.id, info.bars);
            assert_eq!(show.tracks.len(), 4, "{}: four tracks, one per layer", info.id);
            // Every cue id exists, and the show passes the editor's checks.
            for id in show.cue_ids() {
                assert!(presets.iter().any(|p| p.id == id), "{}: unknown cue {id}", info.id);
            }
            show.validate_edit(|id| presets.iter().any(|p| p.id == id)).unwrap();
            assert!(show.markers.len() >= 5, "{}: named sections", info.id);
            assert!(show.markers.iter().all(|m| !m.name.is_empty() && m.at < show.end()));
            // Events never overlap on a track: each section replaces the last.
            for t in &show.tracks {
                for w in t.events.windows(2) {
                    assert!(w[0].start + w[0].len <= w[1].start + 1e-9, "{}: overlap on « {} » at beat {}", info.id, t.name, w[1].start);
                }
            }
        }
        assert!(build("nope", &presets).is_err());
    }

    #[test]
    fn the_dj_entrance_counts_down_4_3_2_1_a_bar_then_black() {
        let presets = catalog();
        let show = build("entree-dj", &presets).unwrap();
        let countdown = presets.iter().find(|p| p.name == "Compte à rebours 4-3-2-1 · blanc").unwrap();
        let Content::Generator { generator, params } = &countdown.settings.content else { panic!("not a generator") };
        assert_eq!(generator, "countdown");
        let mut p = Player::default();
        p.load_example(show);
        assert!(p.play(&clock(0.0, 128.0)));
        for (at_bar, digit) in [(21.0, Some(4)), (22.0, Some(3)), (23.0, Some(2)), (24.0, Some(1)), (24.6, Some(1))] {
            let active = p.frame(&clock(bar(at_bar) + 0.1, 128.0));
            let cue = active.iter().find(|c| c.source == EventSource::Cue { id: countdown.id.clone() }).unwrap_or_else(|| panic!("bar {at_bar}"));
            // The event's own clock: the countdown counts from its start.
            assert!((cue.content_beat - (bar(at_bar) + 0.1 - bar(21.0))).abs() < 1e-6);
            assert_eq!(crate::graphics::countdown_value(params, cue.content_beat), digit, "bar {at_bar}");
        }
        // Beat 4 of bar 24: the event is over, black before the entrance.
        let active = p.frame(&clock(bar(24.75) + 0.03, 128.0));
        assert!(active.iter().all(|c| c.source != EventSource::Cue { id: countdown.id.clone() }));
    }

    #[test]
    fn a_missing_cue_is_an_error_not_a_broken_show() {
        let presets: Vec<Preset> = catalog().into_iter().filter(|p| p.name != "Faisceau chaud · blanc").collect();
        let err = build("montee-drop", &presets).unwrap_err();
        assert!(format!("{err:#}").contains("Faisceau chaud · blanc"));
    }

    /// The player's clock at `beat`, at a fixed tempo.
    fn clock(beat: f64, bpm: f64) -> Clock {
        Clock { t: beat * 60.0 / bpm, beat, bpm, beats_per_bar: 4 }
    }

    #[test]
    fn sections_change_on_the_one_and_blacks_come_before_drops() {
        let presets = catalog();
        for (info, show) in all(&presets) {
            let markers = show.markers.clone();
            let starts: Vec<f64> = show.tracks.iter().flat_map(|t| &t.events).map(|e| e.start).collect();
            let mut p = Player::default();
            p.load_example(show);
            assert!(p.play(&clock(0.0, 128.0)));
            for m in &markers {
                // One frame after the marker (1/60 s at 128 BPM ≈ 0.036 beat).
                let active = p.frame(&clock(m.at + 0.03, 128.0));
                if m.name == "Noir" {
                    assert!(active.is_empty(), "{}: black at beat {}", info.id, m.at);
                } else {
                    assert!(!active.is_empty(), "{}: « {} » plays something", info.id, m.name);
                    assert!(starts.iter().any(|s| (s - m.at).abs() < 1e-9), "{}: « {} » starts something on its beat", info.id, m.name);
                }
            }
            assert!(p.state(&clock(0.0, 128.0)).example);
        }
    }

    #[test]
    fn a_faster_tempo_makes_the_show_shorter_in_proportion() {
        let presets = catalog();
        let show = build("montee-drop", &presets).unwrap();
        let seconds_at = |bpm: f64| {
            let mut p = Player::default();
            p.load_example(show.clone());
            p.play(&clock(0.0, bpm));
            let mut beat = 0.0;
            while p.is_playing() {
                beat += 0.25;
                p.frame(&clock(beat, bpm));
            }
            beat * 60.0 / bpm
        };
        let (s128, s140) = (seconds_at(128.0), seconds_at(140.0));
        assert!((s128 - 32.0 * 4.0 * 60.0 / 128.0).abs() < 0.2, "{s128}");
        assert!((s140 / s128 - 128.0 / 140.0).abs() < 0.01, "{s128} → {s140}");
    }

    #[test]
    fn saving_clears_the_example_flag_and_loading_a_show_too() {
        let presets = catalog();
        let mut p = Player::default();
        p.load_example(build("bloc-techno", &presets).unwrap());
        let c = clock(0.0, 128.0);
        assert!(p.state(&c).example && !p.state(&c).modified, "an example is not an edit");
        p.saved();
        assert!(!p.state(&c).example);
        p.load_example(build("bloc-techno", &presets).unwrap());
        p.load(Show::new_empty("x", TimeBase::Seconds));
        assert!(!p.state(&c).example);
    }

    /// Each example played start to end through the real engine (layers,
    /// budget, live stage, calibration, safety stage, gate) at 250 BPM so
    /// the test stays short: no panic, never armed, every frame within the
    /// point budget and the field, and no layer ever cut.
    #[test]
    fn every_example_renders_through_the_engine_without_arming() {
        let _guard = crate::test_support::time_guard("examples through the engine", Duration::from_secs(240));
        let dir = std::env::temp_dir().join(format!("laser-studio-examples-{}", std::process::id()));
        let mut cut = Vec::new();
        for info in EXAMPLES {
            let shared = Mutex::new(crate::test_support::shared());
            let mut engine = crate::Engine::new(&shared, None, dir.join("live.json"), dir.join("layers.json"));
            let mut now = Instant::now();
            {
                let mut s = shared.lock().unwrap();
                let t = now.saturating_duration_since(s.epoch).as_secs_f64();
                s.tempo.set_bpm(crate::tempo::MAX_BPM, t);
                let show = build(info.id, &s.presets).unwrap();
                s.timeline.load_example(show);
                crate::controls::timeline_play(&mut s).unwrap();
            }
            let (mut frames, mut lit_frames) = (0, 0);
            let step = Duration::from_millis(40);
            let limit = (info.bars as f64 * BPB + 8.0) * 60.0 / crate::tempo::MAX_BPM;
            loop {
                now += step;
                engine.tick(&shared, now);
                frames += 1;
                let s = shared.lock().unwrap();
                assert!(!s.gate.is_armed(), "{}: a show never arms", info.id);
                assert!(s.mix.points <= s.mix.budget, "{}: {} points for {}", info.id, s.mix.points, s.mix.budget);
                if !s.mix.dropped.is_empty() && cut.len() < 40 {
                    let beat = s.timeline.state(&s.timeline_clock()).position;
                    cut.push(format!("{} beat {beat:.1}: layers {:?} cut, {:?} per layer", info.id, s.mix.dropped, s.mix.per_layer));
                }
                for p in &s.frame {
                    assert!(p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 1.0 && p.y.abs() <= 1.0, "{}: {p:?}", info.id);
                    assert!([p.r, p.g, p.b].iter().all(|c| (0.0..=1.0).contains(c)), "{}: {p:?}", info.id);
                }
                if s.frame.iter().any(|p| p.r + p.g + p.b > 0.0) {
                    lit_frames += 1;
                }
                if !s.timeline.is_playing() {
                    break;
                }
                assert!(frames as f64 * step.as_secs_f64() < limit, "{}: still playing after {limit} s", info.id);
            }
            assert!(lit_frames > frames / 2, "{}: lit in {lit_frames} of {frames} frames", info.id);
            engine.shutdown(&shared);
        }
        let _ = std::fs::remove_dir_all(dir);
        assert!(cut.is_empty(), "{}", cut.join("\n"));
    }
}
