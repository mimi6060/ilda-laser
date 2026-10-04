//! One DMX instance per projection zone (T-407): a lighting console
//! patches each zone as a fixture with one of our personalities
//! (personality.rs), at its own universe and start address.
//!
//! Each instance keeps its zone's remote state (`ZoneRemote`), applied to
//! that zone's image after the master live controls and before the zone
//! geometry, the calibration and the safety stage (`apply`): two instances
//! give two different images. The cue, page, animation speed and *arrêter
//! les cues* act on the whole studio, through the gate like every other
//! external write (external.rs).
//!
//! Safety:
//! - Nothing here arms, and no channel reaches a safety setting, a zone's
//!   geometry or the calibration: the remote only shapes a copy of the
//!   master live controls, and the zone geometry and calibration clamp to
//!   -1..1 after it.
//! - *Fonction › noir* latches the emergency stop on the way in, from any
//!   instance, access or not, gate open or not (as the blackout control).
//! - Otherwise an instance acts only with the gate open, its *Accès*
//!   channel ≥ 128, and the DMX scope holding the group (*Maîtres* for the
//!   image, *Cues* for cue, page and stop). Access down, gate closed or
//!   scope missing: its zone goes back to neutral.
//! - Signal loss follows the DMX input's policy (T-406): « neutre » drops
//!   the remote, « garder » keeps it, the fades act on the output gain,
//!   « blackout » latches the stop.

use super::personality::{self, Decoded, Func, Personality, ZoneRemote};
use crate::controls::{self, ControlInput};
use crate::external::{self, SourceKind};
use crate::live::{self, LiveModifiers, LiveState, Palette};
use crate::netin::{LossPolicy, NetInConfig};
use crate::patterns::Point;
use crate::projection::MAX_ZONES;
use crate::Shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const MAX_FIXTURES: usize = MAX_ZONES as usize;
pub const MAX_SMOOTHING_MS: u16 = 1000;
pub const DEFAULT_SMOOTHING_MS: u16 = 60;
const UNIVERSE_LEN: u16 = 512;

/// One instance (`netin.json` → `dmx.fixtures`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DmxFixture {
    pub enabled: bool,
    /// The projection zone's id (projection.rs), 1..=8.
    pub zone: u8,
    pub mode: Personality,
    /// Shared numbering: Art-Net Port-Address N and sACN universe N.
    pub universe: u16,
    /// 1-based start address.
    pub address: u16,
    /// Geometry interpolation between two received frames; 0: none.
    pub smoothing_ms: u16,
}

impl Default for DmxFixture {
    fn default() -> Self {
        DmxFixture { enabled: false, zone: 1, mode: Personality::Compact, universe: 1, address: 1, smoothing_ms: DEFAULT_SMOOTHING_MS }
    }
}

impl DmxFixture {
    pub fn width(&self) -> u16 {
        self.mode.width()
    }

    /// Its last channel.
    pub fn last(&self) -> u16 {
        self.address + self.width() - 1
    }

    pub fn max_address(mode: Personality) -> u16 {
        UNIVERSE_LEN - mode.width() + 1
    }
}

/// What saving refuses (French). `zone_name` names a zone for the messages.
pub fn check(list: &[DmxFixture], zone_name: impl Fn(u8) -> String) -> Result<(), String> {
    if list.len() > MAX_FIXTURES {
        return Err(format!("trop d'appareils DMX ({MAX_FIXTURES} au plus, un par zone)"));
    }
    for (i, f) in list.iter().enumerate() {
        if !(1..=MAX_ZONES).contains(&f.zone) {
            return Err(format!("zone invalide pour un appareil DMX : {} (1 à {MAX_ZONES})", f.zone));
        }
        if f.universe > crate::netin::patch::MAX_UNIVERSE {
            return Err(format!("univers DMX invalide : {} (0 à {})", f.universe, crate::netin::patch::MAX_UNIVERSE));
        }
        let max = DmxFixture::max_address(f.mode);
        if !(1..=max).contains(&f.address) {
            return Err(format!("adresse DMX invalide : {} (1 à {max} en {})", f.address, f.mode.label_fr()));
        }
        if f.smoothing_ms > MAX_SMOOTHING_MS {
            return Err(format!("lissage invalide : {} ms (0 à {MAX_SMOOTHING_MS} ms)", f.smoothing_ms));
        }
        for o in &list[..i] {
            if o.zone == f.zone {
                return Err(format!("la zone « {} » a déjà un appareil DMX", zone_name(f.zone)));
            }
            if o.universe == f.universe && f.address <= o.last() && o.address <= f.last() {
                return Err(format!("Les adresses {}–{} chevauchent l'appareil de la zone « {} ».", f.address, f.last(), zone_name(o.zone)));
            }
        }
    }
    Ok(())
}

/// Geometry interpolation: from where it was to the new target.
#[derive(Clone, Copy, Debug)]
struct Smooth {
    from: [f32; 6],
    to: [f32; 6],
    since: Instant,
    dur: Duration,
}

impl Smooth {
    fn at(&self, now: Instant) -> [f32; 6] {
        if self.dur.is_zero() {
            return self.to;
        }
        let k = (now.saturating_duration_since(self.since).as_secs_f32() / self.dur.as_secs_f32()).clamp(0.0, 1.0);
        std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * k)
    }
}

/// What the last acted-on packet carried (edges).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Acted {
    function: Func,
    page: u8,
    cue: u8,
    speed: f32,
}

#[derive(Debug)]
struct Instance {
    fixture: DmxFixture,
    /// The last packet's channels.
    window: Option<Vec<Option<u8>>>,
    decoded: Option<Decoded>,
    smooth: Option<Smooth>,
    /// Accumulated rotation (the rotation speed channel), degrees.
    spin: f32,
    ticked: Option<Instant>,
    /// What was acted on, while access was up; None: take everything.
    acted: Option<Acted>,
    /// *Fonction* in the previous packet (the blackout's edge).
    function: Func,
}

impl Instance {
    fn new(fixture: DmxFixture) -> Self {
        Instance { fixture, window: None, decoded: None, smooth: None, spin: 0.0, ticked: None, acted: None, function: Func::Nothing }
    }

    /// Forgets the packets (lost, switched off): the next one takes over.
    fn forget(&mut self) {
        (self.window, self.decoded, self.smooth, self.acted, self.spin) = (None, None, None, None, 0.0);
    }
}

/// `NetInState.fixtures`: memory only.
#[derive(Debug, Default)]
pub struct FixtureState {
    instances: Vec<Instance>,
    /// This frame's remotes, one per zone whose instance acts.
    pub remotes: Vec<(u8, ZoneRemote)>,
}

impl FixtureState {
    fn instance(&mut self, f: &DmxFixture) -> &mut Instance {
        let i = match self.instances.iter().position(|i| i.fixture == *f) {
            Some(i) => i,
            None => {
                self.instances.retain(|i| i.fixture.zone != f.zone);
                self.instances.push(Instance::new(f.clone()));
                self.instances.len() - 1
            }
        };
        &mut self.instances[i]
    }
}

/// One universe (the newest packet of the frame): every enabled instance
/// patched on it.
pub fn universe(s: &mut Shared, cfg: &NetInConfig, universe: u16, data: &[u8], now: Instant) {
    let fixtures: Vec<DmxFixture> = cfg.dmx.fixtures.iter().filter(|f| f.enabled && f.universe == universe).cloned().collect();
    for f in fixtures {
        let start = f.address as usize - 1;
        let window: Vec<Option<u8>> = (0..f.width() as usize).map(|i| data.get(start + i).copied()).collect();
        packet(s, &f, window, now);
    }
}

fn packet(s: &mut Shared, f: &DmxFixture, window: Vec<Option<u8>>, now: Instant) {
    let d = personality::decode(f.mode, &window);
    let inst = s.netin.fixtures.instance(f);
    let fresh = inst.window.is_none();
    let blackout = d.function == Func::Blackout && (fresh || inst.function != Func::Blackout);
    inst.function = d.function;
    inst.window = Some(window);
    let target = d.remote.geometry();
    let dur = Duration::from_millis(f.smoothing_ms as u64);
    inst.smooth = Some(match inst.smooth {
        Some(sm) => Smooth { from: sm.at(now), to: target, since: now, dur },
        None => Smooth { from: target, to: target, since: now, dur },
    });
    // Access down: nothing acted on; when it comes back, every channel
    // takes over again.
    let before = if d.access { inst.acted } else { None };
    let now_acted = Acted { function: d.function, page: d.page, cue: d.cue, speed: d.anim_speed };
    inst.acted = d.access.then_some(now_acted);
    inst.decoded = Some(d.clone());

    if blackout {
        // Like the blackout control: accepted from everywhere, always.
        let _ = external::apply(s, SourceKind::Dmx, "", "transport.blackout", ControlInput::Value(1.0), now);
    }
    if !d.access {
        return;
    }
    if d.function == Func::StopCues && before.is_none_or(|b| b.function != Func::StopCues) {
        let _ = external::instance_apply(s, true, "cue.stop_all", ControlInput::Value(1.0));
    }
    if before.is_none_or(|b| b.speed != d.anim_speed) {
        let _ = external::instance_apply(s, true, "master.speed", ControlInput::Value(d.anim_speed));
    }
    if d.cue > 0 && before.is_none_or(|b| (b.page, b.cue) != (d.page, d.cue)) {
        play(s, &d);
    }
}

/// The cue channel: a cell of the instance's page.
fn play(s: &mut Shared, d: &Decoded) {
    let cells = controls::GRID_ROWS * controls::GRID_COLS;
    let page = if d.page == 0 { s.cue_page } else { d.page as usize - 1 };
    if page >= s.grid.page_count() || !(1..=cells).contains(&(d.cue as usize)) {
        return;
    }
    let i = d.cue as usize - 1;
    let cell = format!("{}.{}.{}", page + 1, i / controls::GRID_COLS + 1, i % controls::GRID_COLS + 1);
    if let Some(id) = controls::grid_cell_preset(&s.grid, &cell) {
        let _ = external::instance_play_cue(s, true, &id);
    }
}

/// Once per engine frame, after the packets: each instance's remote at
/// `now` (smoothing, rotation), or none (neutral).
pub fn tick(s: &mut Shared, cfg: &NetInConfig, now: Instant) {
    let on = cfg.artnet.enabled || cfg.sacn.enabled;
    let wanted: Vec<&DmxFixture> = cfg.dmx.fixtures.iter().filter(|f| on && f.enabled).collect();
    s.netin.fixtures.instances.retain(|i| wanted.contains(&&i.fixture));
    let allowed = external::instance_allows(s, "master").is_ok();
    let feeds = &s.netin.patch.feeds;
    let mut remotes = Vec::new();
    for inst in &mut s.netin.fixtures.instances {
        let dt = inst.ticked.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32().min(0.1));
        inst.ticked = Some(now);
        match feeds.get(&inst.fixture.universe) {
            // The protocol was switched off: forgotten.
            None => inst.forget(),
            Some(feed) if feed.lost && cfg.loss.policy == LossPolicy::Neutral => inst.forget(),
            _ => {}
        }
        let (Some(d), Some(sm)) = (&inst.decoded, &inst.smooth) else { continue };
        if !(allowed && d.access) {
            inst.spin = 0.0;
            continue;
        }
        inst.spin = (inst.spin + d.remote.rot_speed * dt).rem_euclid(360.0);
        let mut r = d.remote.clone();
        r.set_geometry(sm.at(now));
        r.rot_deg += inst.spin;
        remotes.push((inst.fixture.zone, r));
    }
    s.netin.fixtures.remotes = remotes;
}

/// The master live controls with a zone's remote on top: dimmer and sizes
/// multiply, position and angle add, a colour mode replaces the master's.
pub fn modifiers(r: &ZoneRemote, master: &LiveModifiers) -> LiveModifiers {
    let mut m = master.clone();
    m.brightness = master.brightness.clamp(0.0, 1.0) * r.brightness.clamp(0.0, 1.0);
    m.size *= r.size;
    m.size_x *= r.size_x;
    m.size_y *= r.size_y;
    m.pos_x += r.pos[0];
    m.pos_y += r.pos[1];
    m.rot_angle[2] += r.rot_deg;
    if r.color_mode != 0 {
        let mut p = master.color_params.clone();
        p.hue = r.hue;
        p.rgb = live::hsv_to_rgb(r.hue / 360.0, 1.0, 1.0).map(|c| (c * 255.0).round() as u8);
        m.color = p.build(r.color_mode);
    }
    m
}

/// A zone's mix through the master live controls and its remote: the
/// image the zone geometry, the calibration and the safety stage get.
pub fn apply(points: &[Point], r: &ZoneRemote, master: &LiveModifiers, st: &LiveState, user: &[Palette], t: f64) -> Vec<Point> {
    let mut out = live::apply(points, &modifiers(r, master), st, user);
    for &(fx, amount) in &r.effects {
        effect(&mut out, fx, amount.clamp(0.0, 1.0), t);
    }
    if r.rgb_mix > 0.0 {
        let (c, k) = (r.rgb.map(|v| v as f32 / 255.0), r.rgb_mix.clamp(0.0, 1.0));
        for p in out.iter_mut().filter(|p| p.is_lit()) {
            let v = p.r.max(p.g).max(p.b);
            (p.r, p.g, p.b) = (p.r + (c[0] * v - p.r) * k, p.g + (c[1] * v - p.g) * k, p.b + (c[2] * v - p.b) * k);
        }
    }
    // Half on, half off; the strobe limiter (T-101) has the last word.
    if r.strobe_hz > 0.0 && (t * r.strobe_hz as f64).rem_euclid(1.0) >= 0.5 {
        for p in &mut out {
            (p.r, p.g, p.b) = (0.0, 0.0, 0.0);
        }
    }
    out
}

/// Our zone effects (`personality::EFFECTS`), on every point (travel
/// follows the lit path). Amount 0..1.
fn effect(points: &mut [Point], fx: u8, a: f32, t: f64) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    if a <= 0.0 {
        return;
    }
    let t = t as f32;
    match fx {
        // Vague: a travelling sine across the figure.
        1 => points.iter_mut().for_each(|p| p.y += a * 0.25 * (TAU * (0.75 * p.x + 0.5 * t)).sin()),
        // Pulsation: the size breathes once a second.
        2 => {
            let k = 1.0 + a * 0.3 * (TAU * t).sin();
            points.iter_mut().for_each(|p| (p.x, p.y) = (p.x * k, p.y * k));
        }
        // Balancement: sways left and right every four seconds.
        3 => {
            let dx = a * 0.4 * (TAU * 0.25 * t).sin();
            points.iter_mut().for_each(|p| p.x += dx);
        }
        // Torsion: the further from the centre, the more it turns.
        4 => points.iter_mut().for_each(|p| {
            let (sin, cos) = (a * FRAC_PI_2 * p.x.hypot(p.y)).sin_cos();
            (p.x, p.y) = (p.x * cos - p.y * sin, p.x * sin + p.y * cos);
        }),
        _ => {}
    }
}

/// « off », « waiting », « ok » or « lost » for an instance.
fn signal(s: &Shared, cfg: &NetInConfig, f: &DmxFixture) -> &'static str {
    if !(f.enabled && (cfg.artnet.enabled || cfg.sacn.enabled)) {
        return "off";
    }
    match s.netin.patch.feeds.get(&f.universe) {
        Some(feed) if feed.lost => "lost",
        Some(_) if s.netin.fixtures.instances.iter().any(|i| i.fixture == *f && i.window.is_some()) => "ok",
        _ => "waiting",
    }
}

/// The instances for `GET /api/netin`: settings, signal, access, whether
/// the zone's image follows it, and its channels' last values.
pub fn view(s: &Shared, cfg: &NetInConfig) -> Value {
    let zones: Vec<Value> = s.projection.get().zones.iter().map(|z| json!({ "id": z.id, "name": z.name })).collect();
    let list: Vec<Value> = cfg
        .dmx
        .fixtures
        .iter()
        .map(|f| {
            let inst = s.netin.fixtures.instances.iter().find(|i| i.fixture == *f);
            let values: Vec<Option<u8>> = inst.and_then(|i| i.window.clone()).unwrap_or_else(|| vec![None; f.width() as usize]);
            json!({
                "fixture": f,
                "zone_name": s.projection.get().zone(f.zone).map(|z| z.name.clone()),
                "last": f.last(),
                "signal": signal(s, cfg, f),
                "access": inst.and_then(|i| i.decoded.as_ref()).is_some_and(|d| d.access),
                "applied": s.netin.fixtures.remotes.iter().any(|(z, _)| *z == f.zone),
                "values": values,
            })
        })
        .collect();
    let personalities: Vec<Value> =
        Personality::ALL.iter().map(|p| json!({ "id": p, "label": p.label_fr(), "width": p.width(), "max_address": DmxFixture::max_address(*p) })).collect();
    json!({ "list": list, "zones": zones, "personalities": personalities, "version": personality::VERSION })
}

/// For the DMX monitor: the instances patched on `universe`, with their
/// channels named.
pub fn monitor(cfg: &NetInConfig, universe: u16) -> Vec<Value> {
    cfg.dmx
        .fixtures
        .iter()
        .filter(|f| f.universe == universe)
        .map(|f| {
            let names: Vec<&str> = f.mode.channels().iter().map(|c| c.label_fr).collect();
            json!({ "zone": f.zone, "mode": f.mode, "label": f.mode.label_fr(), "enabled": f.enabled, "start": f.address, "last": f.last(), "channels": names })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external::{ExternalConfig, SourceScope};
    use crate::interlock::ArmSource;
    use crate::netin::{patch::UniverseEvent, NetEvent, NetHub, NetInState, Proto};
    use crate::projection::{ProjectionConfig, ProjectionZone};
    use crate::test_support::shared;
    use std::sync::Arc;

    fn fx(zone: u8, mode: Personality, address: u16) -> DmxFixture {
        DmxFixture { enabled: true, zone, mode, address, ..DmxFixture::default() }
    }

    fn names(z: u8) -> String {
        ["", "Gauche", "Centre", "Droite"].get(z as usize).unwrap_or(&"?").to_string()
    }

    #[test]
    fn overlapping_ranges_and_bad_addresses_are_refused() {
        check(&[fx(1, Personality::Compact, 1), fx(2, Personality::Full, 17)], names).unwrap();
        let err = check(&[fx(2, Personality::Compact, 30), fx(3, Personality::Full, 17)], names).unwrap_err();
        assert_eq!(err, "Les adresses 17–48 chevauchent l'appareil de la zone « Centre ».");
        // Another universe: fine.
        check(&[fx(2, Personality::Compact, 30), DmxFixture { universe: 2, ..fx(3, Personality::Full, 17) }], names).unwrap();
        // Disabled ones count too (enabling one must not need a re-check).
        assert!(check(&[DmxFixture { enabled: false, ..fx(1, Personality::Compact, 1) }, fx(2, Personality::Compact, 16)], names).is_err());
        assert!(check(&[fx(1, Personality::Compact, 1), fx(1, Personality::Compact, 100)], names).unwrap_err().contains("déjà"));
        assert!(check(&[fx(1, Personality::Full, 482)], names).is_err(), "past the universe");
        check(&[fx(1, Personality::Full, 481)], names).unwrap();
        check(&[fx(1, Personality::Compact, 497)], names).unwrap();
        assert!(check(&[fx(1, Personality::Compact, 0)], names).is_err());
        assert!(check(&[fx(0, Personality::Compact, 1)], names).is_err());
        assert!(check(&[fx(9, Personality::Compact, 1)], names).is_err());
        assert!(check(&[DmxFixture { smoothing_ms: 5000, ..fx(1, Personality::Compact, 1) }], names).is_err());
        // An old netin.json: no fixtures.
        let old: crate::netin::patch::PatchConfig = serde_json::from_str(r#"{"mappings":[]}"#).unwrap();
        assert!(old.fixtures.is_empty());
    }

    #[test]
    fn a_sixteen_bit_sweep_at_30_hz_moves_less_than_half_a_percent_per_frame() {
        // Position X from -1 to +1 over 4 s, sampled by the console at 30
        // Hz in 16 bits; the engine renders at 60 frames per second.
        let t0 = Instant::now();
        let mut sm: Option<Smooth> = None;
        let mut prev: Option<f32> = None;
        let mut max_step = 0.0f32;
        let (mut raw_prev, mut raw_max): (Option<f32>, f32) = (None, 0.0);
        for frame in 0..=240u32 {
            let now = t0 + Duration::from_micros(frame as u64 * 16_667);
            if frame % 2 == 0 {
                let v = (frame as f32 / 240.0 * 65535.0).round() as u16;
                let x = personality::norm16(v, true) * 2.0 - 1.0;
                let target = [1.0, 1.0, 1.0, x, 0.0, 0.0];
                sm = Some(match sm {
                    Some(s) => Smooth { from: s.at(now), to: target, since: now, dur: Duration::from_millis(60) },
                    None => Smooth { from: target, to: target, since: now, dur: Duration::from_millis(60) },
                });
                if let Some(p) = raw_prev {
                    raw_max = raw_max.max((x - p).abs());
                }
                raw_prev = Some(x);
            }
            let x = sm.unwrap().at(now)[3];
            if let Some(p) = prev {
                max_step = max_step.max((x - p).abs());
            }
            prev = Some(x);
        }
        assert!(max_step < 0.01, "smoothed step {max_step}");
        assert!(raw_max > 0.01, "without smoothing it would step: {raw_max}");
        // About one packet behind the console, then on the target.
        assert!((prev.unwrap() - 1.0).abs() < 0.05, "follows: {prev:?}");
        let end = t0 + Duration::from_millis(4100);
        assert_eq!(sm.unwrap().at(end)[3], 1.0, "reaches the end");
    }

    // ---------- in a studio ----------

    fn full_scope() -> ExternalConfig {
        let groups: Vec<String> = external::SCOPE_SETS.iter().flat_map(|set| set.groups.iter().map(|g| g.to_string())).collect();
        ExternalConfig { scopes: SourceKind::ALL.iter().map(|&source| SourceScope { source, groups: groups.clone(), require_access: true }).collect() }
    }

    /// Two zones, sACN on, two instances (Compact at 1 for zone 1, Full at
    /// 17 for zone 2), gate open on every group.
    fn studio() -> Shared {
        let mut s = shared();
        s.netin = NetInState::new(NetHub::in_memory(Arc::clone(&s.estop)));
        let zones = vec![ProjectionZone { id: 1, name: "Gauche".into(), ..Default::default() }, ProjectionZone { id: 2, name: "Droite".into(), ..Default::default() }];
        s.projection.set(ProjectionConfig { zones, default_zones: vec![1, 2] }).unwrap();
        let mut cfg = NetInConfig::default();
        cfg.sacn.enabled = true;
        cfg.dmx.fixtures = vec![fx(1, Personality::Compact, 1), fx(2, Personality::Full, 17)];
        s.netin.hub.set_file(cfg).unwrap();
        s.external.set_config(full_scope());
        external::set_open(&mut s, true, ArmSource::Ui).unwrap();
        s
    }

    fn send(s: &mut Shared, data: &[u8], at: Instant) {
        let ev = UniverseEvent { proto: Proto::Sacn, universe: 1, from: "127.0.0.1".parse().unwrap(), name: "Console".into(), data: data.to_vec() };
        s.netin.sender().try_send(NetEvent::Universe(ev)).unwrap();
        crate::netin::frame(s, at);
    }

    /// A universe with both instances at their defaults, access raised.
    fn universe_data() -> Vec<u8> {
        let mut data = vec![0u8; 64];
        for (i, v) in personality::defaults(Personality::Compact).into_iter().enumerate() {
            data[i] = v.unwrap();
        }
        for (i, v) in personality::defaults(Personality::Full).into_iter().enumerate() {
            data[16 + i] = v.unwrap();
        }
        (data[15], data[16 + 31]) = (255, 255);
        data
    }

    fn remote(s: &Shared, zone: u8) -> Option<ZoneRemote> {
        s.netin.fixtures.remotes.iter().find(|(z, _)| *z == zone).map(|(_, r)| r.clone())
    }

    #[test]
    fn two_instances_drive_two_zones_independently() {
        let mut s = studio();
        let t0 = Instant::now();
        let mut data = universe_data();
        data[2] = 0; // zone 1: X to the left (8 bits)
        (data[16 + 3], data[16 + 4]) = (0xFF, 0xFF); // zone 2: X right (16 bits)
        send(&mut s, &data, t0);
        crate::netin::frame(&mut s, t0 + Duration::from_millis(100));
        let (a, b) = (remote(&s, 1).unwrap(), remote(&s, 2).unwrap());
        assert_eq!((a.pos[0], b.pos[0]), (-1.0, 1.0));
        // Through the projection render: two different images.
        let line: Vec<Point> = (0..=10).map(|i| Point { x: 0.0, y: -0.5 + i as f32 * 0.1, r: 1.0, g: 1.0, b: 1.0 }).collect();
        let master = LiveModifiers::default();
        let st = LiveState::default();
        let remotes = s.netin.fixtures.remotes.clone();
        let finish = |zone: u8, look: &[Point]| match remotes.iter().find(|(z, _)| *z == zone) {
            Some((_, r)) => apply(look, r, &master, &st, &[], 0.0),
            None => live::apply(look, &master, &st, &[]),
        };
        let (frame, _) = crate::projection::render(vec![(1, line, vec![1, 2])], s.projection.get(), None, &crate::layers::Mixer::default(), finish, &crate::engine::Calibration::default());
        let lit: Vec<&Point> = frame.iter().filter(|p| p.is_lit()).collect();
        assert!(lit.iter().any(|p| p.x == -1.0) && lit.iter().any(|p| p.x == 1.0), "one zone left, the other right");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn access_below_128_applies_nothing_but_the_blackout() {
        let mut s = studio();
        let t0 = Instant::now();
        let mut data = universe_data();
        data[15] = 127;
        data[0] = 10; // intensity
        data[11] = 1; // cue
        data[6] = 200; // animation speed
        send(&mut s, &data, t0);
        assert!(remote(&s, 1).is_none(), "neutral");
        assert!(s.deck.active.is_empty() && s.live.speed == 1.0);
        // Raised: the zone follows, the cue plays, the speed changes.
        data[15] = 128;
        send(&mut s, &data, t0);
        assert!((remote(&s, 1).unwrap().brightness - 10.0 / 255.0).abs() < 1e-6);
        assert!(!s.deck.active.is_empty());
        assert!((s.live.speed - 200.0 / 64.0).abs() < 1e-6);
        // Down again: back to neutral at once.
        data[15] = 0;
        send(&mut s, &data, t0);
        assert!(remote(&s, 1).is_none());
        // The blackout passes with access down, and only on the way in.
        data[14] = 200;
        send(&mut s, &data, t0);
        assert!(s.estop.is_latched());
        let estop = Arc::clone(&s.estop);
        s.gate.reset_estop(&estop);
        send(&mut s, &data, t0);
        assert!(!s.estop.is_latched(), "held: no new edge");
        assert!(!s.gate.is_armed());
    }

    #[test]
    fn stop_cues_cue_and_page_through_the_gate() {
        let mut s = studio();
        let t0 = Instant::now();
        let mut data = universe_data();
        data[16 + 21] = 3; // zone 2: cue 3 of the page shown
        send(&mut s, &data, t0);
        let want = controls::grid_cell_preset(&s.grid, "1.1.3").unwrap();
        assert_eq!(s.active_cue.as_deref(), Some(want.as_str()));
        data[16 + 30] = 150; // stop the cues
        send(&mut s, &data, t0);
        assert!(s.deck.active.is_empty());
        // Holding the value doesn't replay the cue nor stop again.
        data[16 + 30] = 0;
        send(&mut s, &data, t0);
        assert!(s.deck.active.is_empty(), "same cue held: not replayed");
        // A scope without the cues: refused, counted.
        let refused = s.external.gate.refused.get(&SourceKind::Dmx).copied().unwrap_or(0);
        let masters = vec![SourceScope { source: SourceKind::Dmx, groups: vec!["master".into()], require_access: true }];
        s.external.set_config(ExternalConfig { scopes: masters });
        data[16 + 21] = 4;
        send(&mut s, &data, t0);
        assert!(s.deck.active.is_empty());
        assert!(s.external.gate.refused[&SourceKind::Dmx] > refused);
        // The gate closed: every zone neutral.
        external::set_open(&mut s, false, ArmSource::Ui).unwrap();
        crate::netin::frame(&mut s, t0);
        assert!(s.netin.fixtures.remotes.is_empty());
    }

    #[test]
    fn a_cue_jump_is_not_smoothed_but_geometry_is() {
        let mut s = studio();
        let t0 = Instant::now();
        let mut data = universe_data();
        send(&mut s, &data, t0);
        data[1] = 255; // zone 1 size to 200 %
        data[10] = 2; // page 2
        data[11] = 1;
        send(&mut s, &data, t0 + Duration::from_millis(100));
        let first = remote(&s, 1).unwrap().size;
        assert!((first - 1.0).abs() < 1e-6, "starts where it was: {first}");
        crate::netin::frame(&mut s, t0 + Duration::from_millis(130));
        let mid = remote(&s, 1).unwrap().size;
        assert!(mid > 1.2 && mid < 1.8, "half-way: {mid}");
        crate::netin::frame(&mut s, t0 + Duration::from_millis(200));
        assert_eq!(remote(&s, 1).unwrap().size, 2.0);
        // The cue of page 2 played at once, in the same frame as the packet.
        let want = controls::grid_cell_preset(&s.grid, "2.1.1");
        assert_eq!(s.active_cue, want);
    }

    #[test]
    fn signal_loss_follows_the_policy() {
        for (policy, kept) in [(LossPolicy::Neutral, false), (LossPolicy::Hold, true)] {
            let mut s = studio();
            let mut cfg = s.netin.hub.file();
            cfg.loss.policy = policy;
            s.netin.hub.set_file(cfg).unwrap();
            let t0 = Instant::now();
            let mut data = universe_data();
            data[0] = 100;
            send(&mut s, &data, t0);
            assert!(remote(&s, 1).is_some());
            crate::netin::frame(&mut s, t0 + Duration::from_secs(3));
            assert_eq!(remote(&s, 1).is_some(), kept, "{policy:?}");
        }
    }

    /// Every value of every channel of both personalities: never armed,
    /// safety settings, zones and calibration untouched.
    #[test]
    fn no_channel_value_ever_arms_or_touches_safety() {
        let mut s = studio();
        let (safety, calibration, zones) = (s.safety.get(), s.calibration, s.projection.get().clone());
        let t0 = Instant::now();
        let base = universe_data();
        for ch in 0..48usize {
            for v in 0..=255u8 {
                let mut data = base.clone();
                data[ch] = v;
                send(&mut s, &data, t0);
                assert!(!s.gate.is_armed(), "channel {} = {v}", ch + 1);
                if s.estop.is_latched() {
                    let estop = Arc::clone(&s.estop);
                    s.gate.reset_estop(&estop);
                    external::set_open(&mut s, true, ArmSource::Ui).unwrap();
                }
            }
        }
        assert!(s.request_arm(ArmSource::Ui).is_ok(), "the gate was armable all along");
        for v in [0u8, 127, 128, 255] {
            send(&mut s, &[v; 64], t0);
        }
        assert_eq!(s.safety.get(), safety);
        assert_eq!(s.calibration, calibration);
        assert_eq!(*s.projection.get(), zones);
    }

    #[test]
    fn neutral_remote_leaves_the_image_as_the_master_draws_it() {
        let pts: Vec<Point> = (0..20).map(|i| Point { x: i as f32 / 20.0 - 0.5, y: 0.3, r: 0.2, g: 0.9, b: 0.4 }).collect();
        let master = LiveModifiers { size: 0.7, pos_x: 0.1, ..Default::default() };
        let st = LiveState::default();
        assert_eq!(apply(&pts, &ZoneRemote::default(), &master, &st, &[], 1.3), live::apply(&pts, &master, &st, &[]));
        // Strobe: dark half of the time; RGB mix: the RGB colour.
        let r = ZoneRemote { strobe_hz: 2.0, ..Default::default() };
        assert!(apply(&pts, &r, &master, &st, &[], 0.3).iter().all(|p| !p.is_lit()));
        assert!(apply(&pts, &r, &master, &st, &[], 0.1).iter().all(|p| p.is_lit()));
        let r = ZoneRemote { rgb: [255, 0, 0], rgb_mix: 1.0, ..Default::default() };
        assert!(apply(&pts, &r, &master, &st, &[], 0.0).iter().all(|p| p.g == 0.0 && p.r > 0.8));
        for fx in 1..=4 {
            let r = ZoneRemote { effects: [(fx, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)], ..Default::default() };
            let out = apply(&pts, &r, &master, &st, &[], 0.37);
            assert_eq!(out.len(), pts.len());
            assert!(out.iter().all(|p| p.x.is_finite() && p.y.is_finite()), "effect {fx}");
        }
    }
}
