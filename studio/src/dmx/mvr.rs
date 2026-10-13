//! The venue as a console or visualiser reads it (T-500): an MVR file
//! (My Virtual Rig, DIN SPEC 15801, public spec at
//! github.com/mvrdevelopment/spec), version 1.6. A `.mvr` is a zip holding
//! `GeneralSceneDescription.xml` and the GDTF files it references: here
//! our one fixture type (gdtf.rs), written stored like the `.gdtf` itself.
//!
//! One `Fixture` per enabled DMX instance (one per projection zone,
//! fixture.rs), at the address the DMX input listens to, placed at the
//! venue projector that draws its zone. Export only: the studio reads no
//! MVR. The venue's screens are left out (an MVR `SceneObject` needs a
//! 3D model file).
//!
//! Coordinates. The venue (venue.rs) is right-handed in metres: x across,
//! y up, z from the audience end (0) to the stage end. MVR is right-handed
//! in millimetres with Z up. We map (x, y, z) → (−x, z, y): a rotation, so
//! MVR's X runs to the audience's right, Y upstage and Z up. A fixture's
//! local axes follow GDTF: X to the projector's right, Y its up and the
//! beam along −Z.
//!
//! UUIDs are derived from stable ids (the zone, the layer), never random:
//! exporting twice gives the same file, so a desk merges instead of
//! duplicating.

use super::fixture::DmxFixture;
use super::gdtf::{self, esc};
use crate::projection::ProjectionConfig;
use crate::venue::{Projector, Venue};
use sha2::{Digest, Sha256};

pub const FILE_NAME: &str = "Laser_Studio.mvr";
pub const SCENE_FILE: &str = "GeneralSceneDescription.xml";
pub const VER_MAJOR: u32 = 1;
pub const VER_MINOR: u32 = 6;

/// A name-based UUID (version 8, RFC 9562) from `name`: SHA-256, the
/// version and variant bits set.
fn stable_uuid(name: &str) -> String {
    let h = Sha256::digest(format!("laser-studio.mvr/{name}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = (b[6] & 0x0F) | 0x80;
    b[8] = (b[8] & 0x3F) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// A venue point (metres) in MVR's frame (millimetres).
pub fn to_mvr_mm(p: [f32; 3]) -> [f64; 3] {
    let [x, y, z] = p.map(f64::from);
    [-x * 1000.0, z * 1000.0, y * 1000.0]
}

/// A venue direction in MVR's frame (no scale).
fn dir(v: [f64; 3]) -> [f64; 3] {
    [-v[0], v[2], v[1]]
}

/// The projector's right / up / forward unit vectors in the venue, as the
/// 3D view computes them (`projectorBasis` in beam3d.js): yaw 0, pitch 0
/// faces the audience (−z), right +x, up +y.
fn basis(p: &Projector) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let r = |deg: f32| f64::from(deg).to_radians();
    let (sy, cy) = r(p.yaw_deg).sin_cos();
    let (sp, cp) = r(p.pitch_deg).sin_cos();
    let (sr, cr) = r(p.roll_deg).sin_cos();
    let f = [sy * cp, sp, -cy * cp];
    let r0 = [cy, 0.0, sy];
    let u0 = [r0[1] * f[2] - r0[2] * f[1], r0[2] * f[0] - r0[0] * f[2], r0[0] * f[1] - r0[1] * f[0]];
    let right = [0, 1, 2].map(|i| cr * r0[i] + sr * u0[i]);
    let up = [0, 1, 2].map(|i| -sr * r0[i] + cr * u0[i]);
    (right, up, f)
}

/// The projector's MVR matrix: rows u, v, w (its local X, Y, Z in MVR
/// axes) and the origin in millimetres. Local −Z is the beam.
pub fn matrix(p: &Projector) -> [[f64; 3]; 4] {
    let (r, u, f) = basis(p);
    [dir(r), dir(u), dir(f.map(|c| -c)), to_mvr_mm(p.pos_m)]
}

fn matrix_text(m: &[[f64; 3]; 4]) -> String {
    // -0.000000 reads badly: add 0.0 to fold the negative zero.
    m.iter().map(|r| format!("{{{:.6},{:.6},{:.6}}}", r[0] + 0.0, r[1] + 0.0, r[2] + 0.0)).collect()
}

/// The projector that draws `zone`: the first one (main first) whose
/// zones include it, as the engine routes them; else the main one.
fn projector_of<'a>(venue: &'a Venue, projection: &ProjectionConfig, selected: Option<&str>, zone: u8) -> Option<&'a Projector> {
    let all: Vec<(&str, Option<&str>)> = venue
        .projectors
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), if i == 0 { selected } else { p.output.as_deref() }))
        .collect();
    venue.projectors.iter().find(|p| projection.zones_for(&p.id, &all).iter().any(|z| z.id == zone)).or(venue.projectors.first())
}

/// `GeneralSceneDescription.xml`: one layer, one fixture per enabled DMX
/// instance. `selected` is the main projector's output (« Sorties et
/// projecteurs »), for the zone routing.
pub fn scene_xml(venue: &Venue, projection: &ProjectionConfig, fixtures: &[DmxFixture], selected: Option<&str>) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    x.push_str(&format!(
        "<GeneralSceneDescription verMajor=\"{VER_MAJOR}\" verMinor=\"{VER_MINOR}\" provider=\"Laser Studio\" providerVersion=\"{}\">\n",
        env!("CARGO_PKG_VERSION")
    ));
    x.push_str("  <Scene>\n    <Layers>\n");
    x.push_str(&format!("      <Layer name=\"Laser Studio\" uuid=\"{}\">\n        <ChildList>\n", stable_uuid("layer")));
    for f in fixtures.iter().filter(|f| f.enabled) {
        let zone = projection.zones.iter().find(|z| z.id == f.zone);
        let name = zone.map_or_else(|| format!("Zone {}", f.zone), |z| z.name.clone());
        let m = projector_of(venue, projection, selected, f.zone).map_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]], matrix);
        x.push_str(&format!("          <Fixture name=\"{}\" uuid=\"{}\">\n", esc(&name), stable_uuid(&format!("zone/{}", f.zone))));
        x.push_str(&format!("            <Matrix>{}</Matrix>\n", matrix_text(&m)));
        x.push_str(&format!("            <GDTFSpec>{}</GDTFSpec>\n", esc(gdtf::FILE_NAME)));
        x.push_str(&format!("            <GDTFMode>{}</GDTFMode>\n", esc(f.mode.name_fr())));
        // MVR universes start at 1: universe 0 (Art-Net 0 only) has no
        // MVR form, so that fixture goes unpatched rather than misplaced.
        if f.universe >= 1 {
            x.push_str(&format!("            <Addresses>\n              <Address break=\"0\">{}.{}</Address>\n            </Addresses>\n", f.universe, f.address));
        }
        x.push_str(&format!("            <FixtureID>{}</FixtureID>\n", f.zone));
        x.push_str(&format!("            <FixtureIDNumeric>{}</FixtureIDNumeric>\n", f.zone));
        x.push_str("            <UnitNumber>0</UnitNumber>\n");
        x.push_str("          </Fixture>\n");
    }
    x.push_str("        </ChildList>\n      </Layer>\n    </Layers>\n  </Scene>\n</GeneralSceneDescription>\n");
    x
}

/// The `.mvr` file: the scene and our GDTF, stored.
pub fn mvr(venue: &Venue, projection: &ProjectionConfig, fixtures: &[DmxFixture], selected: Option<&str>) -> Vec<u8> {
    let scene = scene_xml(venue, projection, fixtures, selected);
    let fixture_type = gdtf::gdtf();
    gdtf::zip_stored(&[(SCENE_FILE, scene.as_bytes()), (gdtf::FILE_NAME, &fixture_type)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dmx::personality::Personality;
    use crate::netin::NetInConfig;
    use crate::projection::ProjectionZone;
    use std::io::Read;

    fn fx(zone: u8, mode: Personality, universe: u16, address: u16) -> DmxFixture {
        DmxFixture { enabled: true, zone, mode, universe, address, ..DmxFixture::default() }
    }

    /// Two projectors; zone 1 on any, zone 2 bound to `p2`.
    fn setup() -> (Venue, ProjectionConfig, Vec<DmxFixture>) {
        let mut venue = Venue::default();
        venue.projectors.truncate(1);
        venue.projectors.push(Projector { id: "p2".into(), name: "Jardin".into(), pos_m: [-3.0, 4.0, 12.0], yaw_deg: 30.0, pitch_deg: -10.0, roll_deg: 5.0, ..Projector::default() });
        let zones = vec![
            ProjectionZone { id: 1, name: "Gauche & <haut>".into(), ..Default::default() },
            ProjectionZone { id: 2, name: "Droite".into(), output: Some("p2".into()), ..Default::default() },
        ];
        let projection = ProjectionConfig { zones, default_zones: vec![1, 2], ..Default::default() };
        let fixtures = vec![fx(1, Personality::Compact, 1, 1), fx(2, Personality::Full, 3, 17), DmxFixture { enabled: false, ..fx(3, Personality::Compact, 1, 100) }];
        (venue, projection, fixtures)
    }

    fn unzip(z: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(z)).expect("a zip");
        (0..a.len())
            .map(|i| {
                let mut f = a.by_index(i).unwrap();
                assert_eq!(f.compression(), zip::CompressionMethod::Stored);
                let mut data = Vec::new();
                f.read_to_end(&mut data).unwrap();
                (f.name().to_string(), data)
            })
            .collect()
    }

    #[test]
    fn the_file_holds_the_scene_and_the_referenced_gdtf() {
        let (venue, projection, fixtures) = setup();
        let files = unzip(&mvr(&venue, &projection, &fixtures, None));
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [SCENE_FILE, gdtf::FILE_NAME]);
        assert_eq!(files[1].1, gdtf::gdtf());
        let xml = std::str::from_utf8(&files[0].1).unwrap();
        let doc = roxmltree::Document::parse(xml).expect("well-formed XML");
        let root = doc.root_element();
        assert_eq!(root.tag_name().name(), "GeneralSceneDescription");
        assert_eq!((root.attribute("verMajor"), root.attribute("verMinor")), (Some("1"), Some("6")));
        assert!(root.attribute("provider").is_some() && root.attribute("providerVersion").is_some());
        let layer = doc.descendants().find(|n| n.has_tag_name("Layer")).unwrap();
        assert_eq!(layer.parent().unwrap().tag_name().name(), "Layers");
        let fixtures: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("Fixture")).collect();
        assert_eq!(fixtures.len(), 2, "enabled instances only");
        let child = |n: &roxmltree::Node, t: &str| n.children().find(|c| c.has_tag_name(t)).and_then(|c| c.text()).map(str::to_string);
        assert_eq!(fixtures[0].attribute("name"), Some("Gauche & <haut>"));
        for (n, mode) in fixtures.iter().zip([Personality::Compact, Personality::Full]) {
            assert_eq!(child(n, "GDTFSpec").as_deref(), Some(gdtf::FILE_NAME), "references the file in the zip");
            assert_eq!(child(n, "GDTFMode").as_deref(), Some(mode.name_fr()));
            for t in ["FixtureID", "FixtureIDNumeric", "UnitNumber", "Matrix"] {
                assert!(child(n, t).is_some(), "{t}");
            }
        }
        // The GDTF declares the modes the scene names.
        let ft = std::str::from_utf8(&unzip(&files[1].1)[0].1).unwrap().to_string();
        let gd = roxmltree::Document::parse(&ft).unwrap();
        let modes: Vec<&str> = gd.descendants().filter(|n| n.has_tag_name("DMXMode")).filter_map(|n| n.attribute("Name")).collect();
        for n in &fixtures {
            assert!(modes.contains(&child(n, "GDTFMode").unwrap().as_str()));
        }
    }

    #[test]
    fn positions_are_millimetres_z_up() {
        assert_eq!(to_mvr_mm([1.0, 3.0, 11.0]), [-1000.0, 11000.0, 3000.0]);
        // Facing the audience, level: the beam (−Z local) goes downstage (−Y).
        let p = Projector { pos_m: [0.5, 2.0, 10.0], ..Projector::default() };
        let m = matrix(&p);
        assert_eq!(m[3], [-500.0, 10000.0, 2000.0]);
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
        assert!(close(m[2], [0.0, 1.0, 0.0]), "w = −beam: {:?}", m[2]);
        assert!(close(m[1], [0.0, 0.0, 1.0]), "its up is Z");
        // Tilted straight down: the beam is −Z.
        let down = matrix(&Projector { pitch_deg: -90.0, ..Projector::default() });
        assert!(close(down[2], [0.0, 0.0, 1.0]), "{:?}", down[2]);
        // Any angles: a rotation (orthonormal, determinant +1).
        let m = matrix(&Projector { yaw_deg: 30.0, pitch_deg: -10.0, roll_deg: 5.0, ..Projector::default() });
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((dot(m[i], m[j]) - want).abs() < 1e-9);
            }
        }
        let (u, v, w) = (m[0], m[1], m[2]);
        let det = u[0] * (v[1] * w[2] - v[2] * w[1]) - u[1] * (v[0] * w[2] - v[2] * w[0]) + u[2] * (v[0] * w[1] - v[1] * w[0]);
        assert!((det - 1.0).abs() < 1e-9);
    }

    #[test]
    fn each_fixture_sits_at_the_projector_drawing_its_zone() {
        let (venue, projection, fixtures) = setup();
        let xml = scene_xml(&venue, &projection, &fixtures, None);
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let mats: Vec<String> = doc.descendants().filter(|n| n.has_tag_name("Matrix")).map(|n| n.text().unwrap().to_string()).collect();
        assert_eq!(mats[0], matrix_text(&matrix(&venue.projectors[0])));
        assert_eq!(mats[1], matrix_text(&matrix(&venue.projectors[1])), "zone 2 is bound to p2");
    }

    #[test]
    fn two_exports_give_the_same_uuids() {
        let (venue, projection, fixtures) = setup();
        let a = mvr(&venue, &projection, &fixtures, None);
        assert_eq!(a, mvr(&venue, &projection, &fixtures, None));
        let xml = scene_xml(&venue, &projection, &fixtures, None);
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let uuids: Vec<&str> = doc.descendants().filter_map(|n| n.attribute("uuid")).collect();
        assert_eq!(uuids.len(), 3);
        for u in &uuids {
            assert_eq!(u.len(), 36);
            assert_eq!(&u[14..15], "8", "version 8");
        }
        assert_ne!(uuids[1], uuids[2]);
        // Moving a projector or renaming a zone keeps the fixture's identity.
        let mut moved = venue.clone();
        moved.projectors[0].pos_m[0] += 1.0;
        let mut renamed = projection.clone();
        renamed.zones[0].name = "Autre".into();
        let xml2 = scene_xml(&moved, &renamed, &fixtures, None);
        let doc2 = roxmltree::Document::parse(&xml2).unwrap();
        assert_eq!(uuids, doc2.descendants().filter_map(|n| n.attribute("uuid")).collect::<Vec<_>>());
    }

    #[test]
    fn the_addresses_are_those_the_dmx_input_listens_to() {
        let (venue, projection, fixtures) = setup();
        let mut cfg = NetInConfig::default();
        cfg.dmx.fixtures = fixtures;
        let mut all = cfg.dmx.fixtures.clone();
        all.push(fx(4, Personality::Compact, 0, 9)); // Art-Net 0: no MVR form
        let xml = scene_xml(&venue, &projection, &all, None);
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let mut checked = 0;
        for n in doc.descendants().filter(|n| n.has_tag_name("Fixture")) {
            let zone: u8 = n.descendants().find(|c| c.has_tag_name("FixtureIDNumeric")).unwrap().text().unwrap().parse().unwrap();
            let Some(addr) = n.descendants().find(|c| c.has_tag_name("Address")) else {
                assert_eq!(zone, 4, "only universe 0 goes unpatched");
                continue;
            };
            assert_eq!(addr.attribute("break"), Some("0"));
            let (u, a) = addr.text().unwrap().split_once('.').unwrap();
            let (u, a): (u16, u16) = (u.parse().unwrap(), a.parse().unwrap());
            // The DMX monitor lists what the input decodes on that universe.
            let listened = crate::dmx::fixture::monitor(&cfg, u);
            let inst = listened.iter().find(|v| v["zone"] == zone).expect("patched on that universe");
            assert_eq!((inst["start"].as_u64(), inst["enabled"].as_bool()), (Some(u64::from(a)), Some(true)));
            checked += 1;
        }
        assert_eq!(checked, 2);
    }
}
