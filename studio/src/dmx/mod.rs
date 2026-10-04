//! Laser Studio as a lighting fixture (T-407): our published DMX
//! personalities (personality.rs), the fixture file a console imports
//! (gdtf.rs) and one instance per projection zone (fixture.rs). The DMX
//! itself arrives through the network inputs (netin/, T-406).

pub mod fixture;
pub mod gdtf;
pub mod personality;
