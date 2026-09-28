//! The boundary between the MIDI worker and the OS. `MidirBackend` talks
//! to CoreMIDI through `midir`; tests use a fake backend, so no unit test
//! ever opens a real port (or lights the user's APC40).

use std::any::Any;

/// Called on a CoreMIDI thread with the raw bytes of each packet. It must
/// not lock anything: the worker's callback only decodes and pushes into a
/// channel.
pub type InputCallback = Box<dyn FnMut(&[u8]) + Send + 'static>;

/// An open input: dropping it closes the port.
pub type InputHandle = Box<dyn Any>;

pub trait OutputPort {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
}

pub trait Backend {
    /// Current input and output port names (made unique, see `unique_names`).
    fn ports(&mut self) -> Result<(Vec<String>, Vec<String>), String>;
    fn open_input(&mut self, name: &str, callback: InputCallback) -> Result<InputHandle, String>;
    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputPort>, String>;
}

/// Two identical controllers get the same CoreMIDI name: the second one
/// becomes "Name (2)" so every port has its own key (and saved settings).
pub fn unique_names(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let mut candidate = name.clone();
        let mut n = 2;
        while out.contains(&candidate) {
            candidate = format!("{name} ({n})");
            n += 1;
        }
        out.push(candidate);
    }
    out
}

const CLIENT: &str = "Laser Studio";

/// Virtual CoreMIDI ports created by our own tests (T-209) start with
/// this. They are visible to every app on the Mac while a test runs, so a
/// running studio never lists (and never opens) them.
pub const TEST_PORT_PREFIX: &str = "Laser Studio Test";

/// CoreMIDI through midir. A fresh client is created for every listing and
/// every connection: midir connections consume their client, and a new
/// client always sees the current device list.
pub struct MidirBackend;

impl MidirBackend {
    /// Every port, our own test ports included (the CoreMIDI tests only).
    pub fn all_ports(&self) -> Result<(Vec<String>, Vec<String>), String> {
        let input = midir::MidiInput::new(CLIENT).map_err(|e| format!("CoreMIDI indisponible : {e}"))?;
        let output = midir::MidiOutput::new(CLIENT).map_err(|e| format!("CoreMIDI indisponible : {e}"))?;
        let inputs = Self::input_ports(&input).into_iter().map(|(n, _)| n).collect();
        let outputs = Self::output_ports(&output).into_iter().map(|(n, _)| n).collect();
        Ok((inputs, outputs))
    }

    fn input_ports(input: &midir::MidiInput) -> Vec<(String, midir::MidiInputPort)> {
        let ports = input.ports();
        let names = ports.iter().map(|p| input.port_name(p).unwrap_or_default()).collect();
        unique_names(names).into_iter().zip(ports).filter(|(n, _)| !n.is_empty()).collect()
    }

    fn output_ports(output: &midir::MidiOutput) -> Vec<(String, midir::MidiOutputPort)> {
        let ports = output.ports();
        let names = ports.iter().map(|p| output.port_name(p).unwrap_or_default()).collect();
        unique_names(names).into_iter().zip(ports).filter(|(n, _)| !n.is_empty()).collect()
    }
}

impl Backend for MidirBackend {
    fn ports(&mut self) -> Result<(Vec<String>, Vec<String>), String> {
        let (inputs, outputs) = self.all_ports()?;
        Ok((without_test_ports(inputs), without_test_ports(outputs)))
    }

    fn open_input(&mut self, name: &str, mut callback: InputCallback) -> Result<InputHandle, String> {
        let mut input = midir::MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
        // SysEx is needed for the Device Inquiry reply; clock for tempo sync.
        input.ignore(midir::Ignore::None);
        let port = Self::input_ports(&input).into_iter().find(|(n, _)| n == name).map(|(_, p)| p).ok_or("port disparu")?;
        let conn = input.connect(&port, "laser-studio-in", move |_stamp, bytes, _| callback(bytes), ()).map_err(|e| e.to_string())?;
        Ok(Box::new(conn))
    }

    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputPort>, String> {
        let output = midir::MidiOutput::new(CLIENT).map_err(|e| e.to_string())?;
        let port = Self::output_ports(&output).into_iter().find(|(n, _)| n == name).map(|(_, p)| p).ok_or("port disparu")?;
        let conn = output.connect(&port, "laser-studio-out").map_err(|e| e.to_string())?;
        Ok(Box::new(MidirOutput(conn)))
    }
}

fn without_test_ports(names: Vec<String>) -> Vec<String> {
    names.into_iter().filter(|n| !n.starts_with(TEST_PORT_PREFIX)).collect()
}

struct MidirOutput(midir::MidiOutputConnection);

impl OutputPort for MidirOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.send(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_port_names_are_numbered() {
        let names = unique_names(vec!["APC40 mkII".into(), "IAC".into(), "APC40 mkII".into(), "APC40 mkII".into()]);
        assert_eq!(names, vec!["APC40 mkII", "IAC", "APC40 mkII (2)", "APC40 mkII (3)"]);
    }

    #[test]
    fn our_own_test_ports_are_never_listed() {
        let names = vec!["APC40 mkII".into(), "Laser Studio Test APC40 mkII".into(), "Laser Studio Test Hotplug (2)".into()];
        assert_eq!(without_test_ports(names), vec!["APC40 mkII"]);
    }
}
