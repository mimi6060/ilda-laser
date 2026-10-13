//! Laser Studio as a lighting fixture (T-407): our published DMX
//! personalities (personality.rs), the fixture file a console imports
//! (gdtf.rs), one instance per projection zone (fixture.rs) and the venue
//! file that places them (mvr.rs, T-500). The DMX itself arrives through
//! the network inputs (netin/, T-406).

pub mod fixture;
pub mod gdtf;
pub mod mvr;
pub mod personality;
