use crate::{
    common::{NesRegion, Reset, ResetKind},
    cpu::{Cpu, Irq},
};
use serde::{Deserialize, Serialize};
use tracing::trace;

/// The APU Frame Counter generates a low-frequency clock for each APU channel.
///
/// See: <https://www.nesdev.org/wiki/APU_Frame_Counter>
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FrameCounter {
    pub region: NesRegion,
    pub step_cycles: [[u16; 6]; 2],
    pub step: usize,
    pub mode: usize,
    pub write_buffer: Option<u8>,
    pub write_delay: u8,
    pub block_counter: u8,
    pub cycle: usize,
    pub inhibit_irq: bool, // Set by $4017 D6
}

/// The Frame Counter clock type.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameType {
    #[default]
    None,
    Quarter,
    Half,
}

impl FrameCounter {
    const STEP_CYCLES_NTSC: [[u16; 6]; 2] = [
        [7457, 14913, 22371, 29828, 29829, 29830],
        [7457, 14913, 22371, 29829, 37281, 37282],
    ];
    const STEP_CYCLES_PAL: [[u16; 6]; 2] = [
        [8313, 16627, 24939, 33252, 33253, 33254],
        [8313, 16627, 24939, 33253, 41565, 41566],
    ];

    const FRAME_TYPE: [FrameType; 6] = [
        FrameType::Quarter,
        FrameType::Half,
        FrameType::Quarter,
        FrameType::None,
        FrameType::Half,
        FrameType::None,
    ];

    pub const fn new(region: NesRegion) -> Self {
        let step_cycles = Self::step_cycles(region);
        Self {
            region,
            step_cycles,
            step: 0,
            mode: 0,
            write_buffer: None,
            write_delay: 0,
            block_counter: 0,
            cycle: 0,
            inhibit_irq: false,
        }
    }

    pub fn set_region(&mut self, region: NesRegion) {
        self.region = region;
        self.step_cycles = Self::step_cycles(region);
    }

    const fn step_cycles(region: NesRegion) -> [[u16; 6]; 2] {
        match region {
            NesRegion::Auto | NesRegion::Ntsc | NesRegion::Dendy => Self::STEP_CYCLES_NTSC,
            NesRegion::Pal => Self::STEP_CYCLES_PAL,
        }
    }

    /// On write to $4017
    pub fn write(&mut self, val: u8, cycle: usize) {
        self.write_buffer = Some(val);
        // Writes occurring on odd clocks are delayed
        self.write_delay = if cycle & 0x01 == 0x01 { 4 } else { 3 };
        trace!("APU $4017 write delay cycles: {}", self.write_delay);
        self.inhibit_irq = val & 0x40 == 0x40; // D6
        if self.inhibit_irq {
            trace!("APU Frame Counter IRQ inhibit");
            Cpu::clear_irq(Irq::FRAME_COUNTER);
        }
    }

    pub fn should_clock(&mut self, cycles: usize) -> bool {
        self.write_buffer.is_some()
            || self.block_counter > 0
            || (self.cycle + cycles) >= (self.step_cycles[self.mode][self.step] - 1) as usize
    }

    // mode 0: 4-step  effective rate (approx)
    // ---------------------------------------
    // - - - f f f      60 Hz
    // - l - - l -     120 Hz
    // e e e - e -     240 Hz
    //
    // mode 1: 5-step  effective rate (approx)
    // ---------------------------------------
    // - - - - - -     (interrupt flag never set)
    // - l - - l -     96 Hz
    // e e e - e -     192 Hz
    pub fn clock_with(&mut self, cycles: usize, mut on_clock: impl FnMut(FrameType)) -> usize {
        let mut cycles_ran = 0;
        let step_cycles = self.step_cycles[self.mode][self.step] as usize;
        if self.cycle + cycles >= step_cycles {
            if !self.inhibit_irq && self.mode == 0 && self.step >= 3 {
                trace!(
                    "APU Frame Counter IRQ pending - cycles: {} >= {step_cycles}",
                    self.cycle + cycles
                );
                Cpu::set_irq(Irq::FRAME_COUNTER);
            }

            let ty = Self::FRAME_TYPE[self.step];
            if ty != FrameType::None && self.block_counter == 0 {
                on_clock(ty);
                // Do not allow writes to $4017 to clock for the next cycle (odd + following even
                // cycle)
                self.block_counter = 2;
            }

            if step_cycles >= self.cycle {
                cycles_ran = step_cycles - self.cycle;
            }

            self.step += 1;
            if self.step == 6 {
                trace!(
                    "APU Frame Counter total cycles: {}",
                    self.cycle + cycles_ran
                );
                self.step = 0;
                self.cycle = 0;
            } else {
                self.cycle += cycles_ran;
            }
        } else {
            cycles_ran = cycles;
            self.cycle += cycles_ran;
        }

        if let Some(val) = self.write_buffer {
            self.write_delay -= 1;
            if self.write_delay == 0 {
                self.mode = if val & 0x80 == 0x80 { 1 } else { 0 };
                self.step = 0;
                self.cycle = 0;
                self.write_buffer = None;
                if self.mode == 1 && self.block_counter == 0 {
                    // Writing to $4017 with bit 7 set will immediately generate a quarter/half frame
                    on_clock(FrameType::Half);
                    self.block_counter = 2;
                }
            }
        }

        if self.block_counter > 0 {
            self.block_counter -= 1;
        }

        cycles_ran
    }
}

impl Reset for FrameCounter {
    fn reset(&mut self, kind: ResetKind) {
        self.cycle = 0;
        if kind == ResetKind::Hard {
            self.mode = 0;
            // After reset, APU acts as if $4017 was written 9-12 clocks before first instruction,
            // Reset acts as if $00 was written to $4017
            self.write(0x00, 0);
            self.write_delay -= 1; // FIXME: Startup timing is slightly wrong, reset_timing fails
                                   // with the default
        }
        self.step = 0;
        self.block_counter = 0;
    }
}
