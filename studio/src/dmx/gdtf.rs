//! The fixture file a console imports (T-407): GDTF 1.2 (DIN SPEC 15800,
//! the open General Device Type Format), generated from the same tables as
//! the receiver (personality.rs). A `.gdtf` file is a zip holding
//! `description.xml`; we write it stored (no compression), by hand.
//!
//! One fixture type, one DMX mode per personality. Every attribute it
//! uses is defined in the file itself.

use super::personality::{ChanKind, ChannelDef, Param, Personality, VERSION};

/// Our fixture type's id: fixed, so a desk sees updates of the same type.
pub const FIXTURE_TYPE_ID: &str = "6C5A1D07-4A0E-4E57-8D1B-0407D3A5F001";
pub const FILE_NAME: &str = "Laser_Studio@Laser_Studio_DMX@v1.gdtf";

/// (attribute, pretty name, feature group, feature, physical unit).
fn attribute(p: Param) -> (String, &'static str, &'static str, &'static str, &'static str) {
    let (name, pretty, group, feature, unit) = match p {
        Param::Intensity => ("Dimmer", "Dim", "Dimmer", "Dimmer", "LuminousIntensity"),
        Param::Size => ("Scale_XYZ", "Taille", "Position", "Scale", "Percent"),
        Param::PosX => ("XYZ_X", "Pos X", "Position", "XYZ", "Length"),
        Param::PosY => ("XYZ_Y", "Pos Y", "Position", "XYZ", "Length"),
        Param::Rotation => ("Rot_Z", "Rot", "Position", "Rotation", "Angle"),
        Param::SizeX => ("Scale_X", "Taille X", "Position", "Scale", "Percent"),
        Param::SizeY => ("Scale_Y", "Taille Y", "Position", "Scale", "Percent"),
        Param::RotSpeed => ("Rot_Z_Speed", "Rot vit.", "Position", "Rotation", "AngularSpeed"),
        Param::AnimSpeed => ("PlaySpeed", "Vit. anim", "Control", "Control", "None"),
        Param::ColorMode => ("ColorMacro1", "Mode coul.", "Color", "Color", "None"),
        Param::Hue => ("HSB_Hue", "Teinte", "Color", "HSB", "Angle"),
        Param::Red => ("ColorRGB_Red", "R", "Color", "RGB", "ColorComponent"),
        Param::Green => ("ColorRGB_Green", "G", "Color", "RGB", "ColorComponent"),
        Param::Blue => ("ColorRGB_Blue", "B", "Color", "RGB", "ColorComponent"),
        Param::RgbMix => ("ColorRGB_Mix", "Dosage RVB", "Color", "RGB", "Percent"),
        Param::Strobe => ("Shutter1Strobe", "Strobe", "Beam", "Beam", "Frequency"),
        Param::Page => ("MediaFolder1", "Page", "Control", "Media", "None"),
        Param::Cue => ("MediaContent1", "Cue", "Control", "Media", "None"),
        Param::Effect(_) | Param::EffectAmount(_) => ("", "", "Control", "Effects", "None"),
        Param::Function => ("Function", "Fonction", "Control", "Control", "None"),
        Param::Access => ("Control1", "Accès", "Control", "Control", "None"),
    };
    let name = match p {
        Param::Effect(i) => format!("Effects{}", i + 1),
        Param::EffectAmount(i) => format!("Effects{}Adjust1", i + 1),
        _ => name.to_string(),
    };
    let pretty = match p {
        Param::Effect(_) => "Effet",
        Param::EffectAmount(_) => "Dosage effet",
        _ => pretty,
    };
    (name, pretty, group, feature, unit)
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Every parameter used by a table, coarse channels only (a 16-bit value
/// is one DMX channel element with two offsets).
fn params(mode: Personality) -> impl Iterator<Item = &'static ChannelDef> {
    mode.channels().iter().filter(|d| d.kind != ChanKind::Fine)
}

fn dmx_channel(mode: Personality, d: &ChannelDef, out: &mut String) {
    let table = mode.channels();
    let fine = table.iter().find(|f| f.kind == ChanKind::Fine && f.param == d.param);
    let (offset, bytes, default) = match fine {
        Some(f) => (format!("{},{}", d.offset + 1, f.offset + 1), 2, (u32::from(d.default) << 8) | u32::from(f.default)),
        None => (format!("{}", d.offset + 1), 1, u32::from(d.default)),
    };
    let (attr, ..) = attribute(d.param);
    let func = esc(d.label_fr);
    out.push_str(&format!(
        "        <DMXChannel DMXBreak=\"1\" Offset=\"{offset}\" InitialFunction=\"Body_{attr}.{attr}.{func}\" Highlight=\"None\" Geometry=\"Body\">\n"
    ));
    out.push_str(&format!("          <LogicalChannel Attribute=\"{attr}\" Snap=\"No\" Master=\"None\" MibFade=\"0\" DMXChangeTimeLimit=\"0\">\n"));
    out.push_str(&format!(
        "            <ChannelFunction Name=\"{func}\" Attribute=\"{attr}\" OriginalAttribute=\"\" DMXFrom=\"0/{bytes}\" Default=\"{default}/{bytes}\" PhysicalFrom=\"0\" PhysicalTo=\"1\" RealFade=\"0\" RealAcceleration=\"0\">\n"
    ));
    // A channel set runs from its DMXFrom to the next one's (ranges are
    // 8-bit channels only).
    for r in d.ranges {
        out.push_str(&format!("              <ChannelSet Name=\"{}\" DMXFrom=\"{}/{bytes}\" WheelSlotIndex=\"0\"/>\n", esc(r.label_fr), r.from));
    }
    out.push_str("            </ChannelFunction>\n          </LogicalChannel>\n        </DMXChannel>\n");
}

/// `description.xml`.
pub fn description_xml() -> String {
    let mut attrs: Vec<(String, &str, &str, &str, &str)> = Vec::new();
    for mode in Personality::ALL {
        for d in params(mode) {
            let a = attribute(d.param);
            if !attrs.iter().any(|o| o.0 == a.0) {
                attrs.push(a);
            }
        }
    }
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for (_, _, g, f, _) in &attrs {
        match groups.iter_mut().find(|(n, _)| n == g) {
            Some((_, fs)) if !fs.contains(f) => fs.push(f),
            Some(_) => {}
            None => groups.push((g, vec![f])),
        }
    }
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<GDTF DataVersion=\"1.2\">\n");
    x.push_str(&format!(
        "  <FixtureType Name=\"Laser Studio DMX\" ShortName=\"LaserStudio\" LongName=\"Laser Studio — zone de projection\" Manufacturer=\"Laser Studio\" Description=\"Une zone de projection de Laser Studio (table v{VERSION}). Aucun canal n'arme le laser.\" FixtureTypeID=\"{FIXTURE_TYPE_ID}\" Thumbnail=\"\" RefFT=\"\" CanHaveChildren=\"No\">\n"
    ));
    x.push_str("    <AttributeDefinitions>\n      <ActivationGroups/>\n      <FeatureGroups>\n");
    for (g, fs) in &groups {
        x.push_str(&format!("        <FeatureGroup Name=\"{g}\" Pretty=\"{g}\">\n"));
        for f in fs {
            x.push_str(&format!("          <Feature Name=\"{f}\"/>\n"));
        }
        x.push_str("        </FeatureGroup>\n");
    }
    x.push_str("      </FeatureGroups>\n      <Attributes>\n");
    for (name, pretty, g, f, unit) in &attrs {
        x.push_str(&format!("        <Attribute Name=\"{name}\" Pretty=\"{}\" Feature=\"{g}.{f}\" PhysicalUnit=\"{unit}\"/>\n", esc(pretty)));
    }
    x.push_str("      </Attributes>\n    </AttributeDefinitions>\n    <Wheels/>\n    <PhysicalDescriptions/>\n");
    x.push_str("    <Models>\n      <Model Name=\"Body\" Length=\"0.3\" Width=\"0.3\" Height=\"0.2\" PrimitiveType=\"Cube\"/>\n    </Models>\n");
    x.push_str("    <Geometries>\n      <Geometry Name=\"Body\" Model=\"Body\" Position=\"{1,0,0,0}{0,1,0,0}{0,0,1,0}{0,0,0,1}\"/>\n    </Geometries>\n");
    x.push_str("    <DMXModes>\n");
    for mode in Personality::ALL {
        x.push_str(&format!("      <DMXMode Name=\"{}\" Description=\"{}\" Geometry=\"Body\">\n        <DMXChannels>\n", mode.name_fr(), esc(&mode.label_fr())));
        for d in params(mode) {
            dmx_channel(mode, d, &mut x);
        }
        x.push_str("        </DMXChannels>\n        <Relations/>\n        <FTMacros/>\n      </DMXMode>\n");
    }
    x.push_str("    </DMXModes>\n    <Revisions>\n");
    x.push_str(&format!("      <Revision Text=\"Table v{VERSION}, générée par Laser Studio\" UserID=\"0\"/>\n"));
    x.push_str("    </Revisions>\n    <FTPresets/>\n    <Protocols/>\n  </FixtureType>\n</GDTF>\n");
    x
}

/// CRC-32 (IEEE), as zip wants it.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A zip archive of stored (uncompressed) files.
pub fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    // 1980-01-01 00:00 (DOS time): a fixed date, so the file is reproducible.
    let (time, date) = (0u16, (1 << 5) | 1u16);
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let (size, nlen) = (data.len() as u32, name.len() as u16);
        let common = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes()); // version needed
            v.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            v.extend_from_slice(&0u16.to_le_bytes()); // stored
            v.extend_from_slice(&time.to_le_bytes());
            v.extend_from_slice(&date.to_le_bytes());
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&nlen.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // extra
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        common(&mut central);
        central.extend_from_slice(&[0; 6]); // comment len, disk, internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let (cd_offset, cd_len, n) = (out.len() as u32, central.len() as u32, files.len() as u16);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The `.gdtf` file.
pub fn gdtf() -> Vec<u8> {
    zip_stored(&[("description.xml", description_xml().as_bytes())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn the_description_is_well_formed_and_matches_the_tables() {
        let xml = description_xml();
        let doc = roxmltree::Document::parse(&xml).expect("well-formed XML");
        let root = doc.root_element();
        assert_eq!((root.tag_name().name(), root.attribute("DataVersion")), ("GDTF", Some("1.2")));
        let ft = root.children().find(|n| n.has_tag_name("FixtureType")).unwrap();
        assert_eq!(ft.attribute("FixtureTypeID"), Some(FIXTURE_TYPE_ID));
        let defined: Vec<&str> = doc.descendants().filter(|n| n.has_tag_name("Attribute")).filter_map(|n| n.attribute("Name")).collect();
        let modes: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("DMXMode")).collect();
        assert_eq!(modes.len(), 2);
        for (node, mode) in modes.iter().zip(Personality::ALL) {
            assert_eq!(node.attribute("Name"), Some(mode.name_fr()));
            let channels: Vec<_> = node.descendants().filter(|n| n.has_tag_name("DMXChannel")).collect();
            // DMX footprint: every offset listed once, 1..=width.
            let mut offsets: Vec<u16> = channels.iter().flat_map(|c| c.attribute("Offset").unwrap().split(',').map(|o| o.parse::<u16>().unwrap())).collect();
            offsets.sort_unstable();
            assert_eq!(offsets, (1..=mode.width()).collect::<Vec<_>>(), "{mode:?}");
            for c in &channels {
                let lc = c.children().find(|n| n.has_tag_name("LogicalChannel")).unwrap();
                assert!(defined.contains(&lc.attribute("Attribute").unwrap()), "every attribute is defined");
                let f = lc.children().find(|n| n.has_tag_name("ChannelFunction")).unwrap();
                let bytes = c.attribute("Offset").unwrap().split(',').count();
                assert!(f.attribute("Default").unwrap().ends_with(&format!("/{bytes}")));
            }
        }
    }

    #[test]
    fn the_gdtf_file_is_a_zip_with_the_description() {
        let z = gdtf();
        assert_eq!(&z[..4], b"PK\x03\x04");
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
        let name_len = u16::from_le_bytes([z[26], z[27]]) as usize;
        assert_eq!(&z[30..30 + name_len], b"description.xml");
        let size = u32::from_le_bytes([z[18], z[19], z[20], z[21]]) as usize;
        let data = &z[30 + name_len..30 + name_len + size];
        assert_eq!(data, description_xml().as_bytes());
        assert_eq!(u32::from_le_bytes([z[14], z[15], z[16], z[17]]), crc32(data));
        assert_eq!(gdtf(), z, "reproducible");
        // A zip reader sees exactly that file through the central directory.
        let mut a = zip::ZipArchive::new(std::io::Cursor::new(&z)).expect("a valid zip");
        assert_eq!(a.len(), 1);
        let mut f = a.by_index(0).unwrap();
        assert_eq!(f.name(), "description.xml");
        let mut read = String::new();
        std::io::Read::read_to_string(&mut f, &mut read).unwrap();
        assert_eq!(read, description_xml());
    }
}
