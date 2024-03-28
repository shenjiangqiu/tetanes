use crate::common::Clock;
use serde::{Deserialize, Serialize};

/// APU Envelope provides volume control for APU waveform channels.
///
/// See: <https://www.nesdev.org/wiki/APU_Envelope>
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[must_use]
pub struct Envelope {
    pub enabled: bool,
    pub loops: bool,
    pub reset: bool,
    pub volume: u8,
    pub constant_volume: u8,
    pub counter: u8,
}

impl Envelope {
    pub const fn new() -> Self {
        Self {
            enabled: false,
            loops: false,
            reset: false,
            volume: 0u8,
            constant_volume: 0u8,
            counter: 0u8,
        }
    }

    /// $4000/$4004/$400C Envelope control
    pub fn write_ctrl(&mut self, val: u8) {
        self.loops = (val >> 5) & 1 == 1; // D5
        self.enabled = (val >> 4) & 1 == 0; // !D4
        self.constant_volume = val & 0x0F; // D3..D0
    }
}

impl Clock for Envelope {
    fn clock(&mut self) -> usize {
        if self.reset {
            self.reset = false;
            self.volume = 0x0F;
            self.counter = self.constant_volume;
        } else if self.counter > 0 {
            self.counter -= 1;
        } else {
            self.counter = self.constant_volume;
            if self.volume > 0 {
                self.volume -= 1;
            } else if self.loops {
                self.volume = 0x0F;
            }
        }
        1
    }
}
