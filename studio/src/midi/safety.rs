//! MIDI safety rules (T-208). A controller can be bumped without anyone
//! looking at the screen, so:
//! - blackout from MIDI always wins (handled first in each batch);
//! - arming from MIDI is off unless the user opts in, and even then needs
//!   Shift + a 1 s hold of the arm button, never in the first 5 s after
//!   the controller is plugged in;
//! - brightness from MIDI is capped and the brightness faders always use
//!   pickup, so a fader left at the top can't make the beam jump.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Global MIDI safety options, saved in `<data-dir>/midi/devices.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiSafety {
    /// « Autoriser l'armement depuis le contrôleur (Shift + maintien 1 s) ».
    #[serde(default)]
    pub allow_arm: bool,
    /// « Blackout si le contrôleur se déconnecte ».
    #[serde(default)]
    pub blackout_on_disconnect: bool,
}

/// How long Shift + the arm button must be held.
pub const ARM_HOLD: Duration = Duration::from_secs(1);
/// No arming this long after a controller is plugged in.
pub const PLUG_GUARD: Duration = Duration::from_secs(5);
/// Highest brightness a controller may set. The global safety maximum
/// (T-003) doesn't exist yet; until then it is full brightness.
pub const BRIGHTNESS_MAX: f32 = 1.0;

pub const ARM: &str = "transport.arm";
pub const BLACKOUT: &str = "transport.blackout";

/// Brightness controls: capped, and always in pickup.
pub fn is_brightness(canonical_id: &str) -> bool {
    matches!(canonical_id, "master.brightness" | "look.brightness")
}
