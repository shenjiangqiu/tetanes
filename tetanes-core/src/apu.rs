use crate::{
    apu::{
        dmc::Dmc,
        filter::{Consume, FilterChain},
        frame_counter::{FrameCounter, FrameType},
        noise::Noise,
        pulse::{OutputFreq, Pulse, PulseChannel},
        triangle::Triangle,
    },
    common::{ClockTo, NesRegion, Regional, Reset, ResetKind, Sample},
    cpu::{Cpu, Irq},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

pub mod dmc;
pub mod noise;
pub mod pulse;
pub mod triangle;

pub mod divider;
pub mod envelope;
pub mod filter;
pub mod frame_counter;
pub mod length_counter;
pub mod timer;

/// Error when parsing `Channel` from a `usize`.
#[derive(Error, Debug)]
#[must_use]
#[error("failed to parse `Channel`")]
pub struct ParseChannelError;

/// APU Channel.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[must_use]
pub enum Channel {
    Pulse1,
    Pulse2,
    Triangle,
    Noise,
    Dmc,
    Mapper,
}

impl TryFrom<usize> for Channel {
    type Error = ParseChannelError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Pulse1),
            1 => Ok(Self::Pulse2),
            2 => Ok(Self::Triangle),
            3 => Ok(Self::Noise),
            4 => Ok(Self::Dmc),
            _ => Err(ParseChannelError),
        }
    }
}

/// Trait for APU registers.
pub trait ApuRegisters {
    fn write_ctrl(&mut self, channel: Channel, val: u8);
    fn write_sweep(&mut self, channel: Channel, val: u8);
    fn write_timer_lo(&mut self, channel: Channel, val: u8);
    fn write_timer_hi(&mut self, channel: Channel, val: u8);
    fn write_linear_counter(&mut self, val: u8);
    fn write_length(&mut self, channel: Channel, val: u8);
    fn write_dmc_output(&mut self, val: u8);
    fn write_dmc_addr(&mut self, val: u8);
    fn read_status(&mut self) -> u8;
    fn peek_status(&self) -> u8;
    fn write_status(&mut self, val: u8);
    fn write_frame_counter(&mut self, val: u8);
}

/// NES APU.
///
/// See: <https://wiki.nesdev.com/w/index.php/APU>
#[derive(Clone, Serialize, Deserialize)]
#[must_use]
pub struct Apu {
    pub frame_counter: FrameCounter,
    pub master_cycle: usize,
    pub cpu_cycle: usize,
    pub cycle: usize,
    pub clock_rate: f32,
    pub region: NesRegion,
    pub pulse1: Pulse,
    pub pulse2: Pulse,
    pub triangle: Triangle,
    pub noise: Noise,
    pub dmc: Dmc,
    pub filter_chain: FilterChain,
    #[serde(skip)]
    pub channel_outputs: Vec<f32>,
    #[serde(skip)]
    pub output_cycle: usize,
    #[serde(skip)]
    pub audio_samples: Vec<f32>,
    pub sample_period: f32,
    pub sample_counter: f32,
    pub mapper_silenced: bool,
    pub skip_mixing: bool,
    pub should_clock: bool,
}

impl Apu {
    pub const SAMPLE_RATE: f32 = 44_100.0;
    // 5 APU channels + 1 Mapper channel
    pub const MAX_CHANNEL_COUNT: usize = 6;
    pub const CYCLE_SIZE: usize = 10_000;

    /// Create a new APU instance.
    pub fn new() -> Self {
        let region = NesRegion::default();
        let clock_rate = Cpu::region_clock_rate(region);
        let sample_period = clock_rate / Self::SAMPLE_RATE;
        Self {
            frame_counter: FrameCounter::new(region),
            master_cycle: 0,
            cpu_cycle: 0,
            cycle: 0,
            clock_rate,
            region,
            pulse1: Pulse::new(PulseChannel::One, OutputFreq::Default),
            pulse2: Pulse::new(PulseChannel::Two, OutputFreq::Default),
            triangle: Triangle::new(),
            noise: Noise::new(region),
            dmc: Dmc::new(region),
            filter_chain: FilterChain::new(region, Self::SAMPLE_RATE),
            channel_outputs: vec![0.0; Self::MAX_CHANNEL_COUNT * Self::CYCLE_SIZE],
            output_cycle: 0,
            audio_samples: Vec::with_capacity((Self::SAMPLE_RATE / 60.0) as usize),
            sample_period,
            sample_counter: sample_period,
            mapper_silenced: true,
            skip_mixing: false,
            should_clock: false,
        }
    }

    pub fn add_output(&mut self, output: f32) {
        let offset = Channel::Mapper as usize;
        self.channel_outputs[(self.master_cycle * Self::MAX_CHANNEL_COUNT) + offset] = output;
    }

    /// Filter and mix audio sample based on region sampling rate.
    pub fn process_outputs(&mut self) {
        if self.skip_mixing {
            return;
        }

        for outputs in self
            .channel_outputs
            .chunks_exact(Self::MAX_CHANNEL_COUNT)
            .take(self.cycle)
        {
            let [pulse1, pulse2, triangle, noise, dmc, mapper] = outputs else {
                warn!("invalid channel outputs");
                return;
            };
            let pulse_idx = (pulse1 + pulse2) as usize;
            let tnd_idx = (3.0f32.mul_add(*triangle, 2.0 * noise) + dmc) as usize;
            let apu_output = PULSE_TABLE[pulse_idx] + TND_TABLE[tnd_idx];
            let mapper_output = if self.mapper_silenced { 0.0 } else { *mapper };

            self.filter_chain.consume(apu_output + mapper_output);
            self.sample_counter -= 1.0;
            if self.sample_counter <= 1.0 {
                self.audio_samples.push(self.filter_chain.output());
                self.sample_counter += self.sample_period;
            }
        }
    }

    /// Set the frame speed of the APU, which affects the sampling rate.
    pub fn set_frame_speed(&mut self, speed: f32) {
        let clock_rate = Cpu::region_clock_rate(self.region);
        let sample_rate = Self::SAMPLE_RATE / speed;
        self.filter_chain = FilterChain::new(self.region, sample_rate);
        self.sample_period = clock_rate / sample_rate;
    }

    /// Whether a given channel is enabled.
    #[must_use]
    pub const fn channel_enabled(&self, channel: Channel) -> bool {
        match channel {
            Channel::Pulse1 => !self.pulse1.silent(),
            Channel::Pulse2 => !self.pulse2.silent(),
            Channel::Triangle => !self.triangle.silent(),
            Channel::Noise => !self.noise.silent(),
            Channel::Dmc => !self.dmc.silent(),
            Channel::Mapper => !self.mapper_silenced,
        }
    }

    /// Enable or disable a given channel.
    pub fn set_channel_enabled(&mut self, channel: Channel, enabled: bool) {
        match channel {
            Channel::Pulse1 => self.pulse1.set_silent(!enabled),
            Channel::Pulse2 => self.pulse2.set_silent(!enabled),
            Channel::Triangle => self.triangle.set_silent(!enabled),
            Channel::Noise => self.noise.set_silent(!enabled),
            Channel::Dmc => self.dmc.set_silent(!enabled),
            Channel::Mapper => self.mapper_silenced = !enabled,
        }
    }

    /// Toggle a given channel.
    pub fn toggle_channel(&mut self, channel: Channel) {
        match channel {
            Channel::Pulse1 => self.pulse1.set_silent(!self.pulse1.silent()),
            Channel::Pulse2 => self.pulse2.set_silent(!self.pulse2.silent()),
            Channel::Triangle => self.triangle.set_silent(!self.triangle.silent()),
            Channel::Noise => self.noise.set_silent(!self.noise.silent()),
            Channel::Dmc => self.dmc.set_silent(!self.dmc.silent()),
            Channel::Mapper => self.mapper_silenced = !self.mapper_silenced,
        }
    }

    /// Whether the APU has any IRQs pending.
    #[inline]
    pub fn irqs_pending(&self) -> Irq {
        let mut irq = Irq::empty();
        irq.set(Irq::FRAME_COUNTER, self.frame_counter.irq_pending);
        irq.set(Irq::DMC, self.dmc.irq_pending);
        irq
    }

    /// Whether the APU DMC is requesting a direct-memory access (DMA) transfer.
    #[inline]
    #[must_use]
    pub fn dmc_dma(&mut self) -> bool {
        self.dmc.dma()
    }

    /// Get the current DMC DMA address.
    #[inline]
    #[must_use]
    pub fn dmc_dma_addr(&self) -> u16 {
        self.dmc.dma_addr()
    }

    /// Load a byte into the DMC buffer.
    #[inline]
    pub fn load_dmc_buffer(&mut self, val: u8) {
        self.dmc.load_buffer(val);
    }

    fn should_clock(&mut self) -> bool {
        if self.dmc.should_clock() || self.should_clock {
            self.should_clock = false;
            return true;
        }
        let cycles = self.master_cycle - self.cycle;
        self.frame_counter.should_clock(cycles) || self.dmc.irq_pending_in(cycles)
    }

    pub fn clock_flush(&mut self) -> usize {
        let cycles = self.clock_to(self.master_cycle);

        self.process_outputs();

        self.master_cycle = 0;
        self.cycle = 0;
        self.pulse1.timer.cycle = 0;
        self.pulse2.timer.cycle = 0;
        self.triangle.timer.cycle = 0;
        self.noise.timer.cycle = 0;
        self.dmc.timer.cycle = 0;

        cycles
    }

    pub fn clock_lazy(&mut self) -> usize {
        self.master_cycle += 1;
        self.cpu_cycle += 1;
        if self.master_cycle == Self::CYCLE_SIZE - 1 {
            self.clock_flush()
        } else if self.should_clock() {
            self.clock_to(self.master_cycle)
        } else {
            0
        }
    }
}

impl ClockTo for Apu {
    fn clock_to(&mut self, cycle: usize) -> usize {
        self.master_cycle = cycle;

        let mut cycles = self.master_cycle - self.cycle;
        while cycles > 0 {
            self.cycle += self
                .frame_counter
                .clock_to_with(&mut cycles, |ty| match ty {
                    FrameType::Quarter => {
                        self.pulse1.clock_quarter_frame();
                        self.pulse2.clock_quarter_frame();
                        self.triangle.clock_quarter_frame();
                        self.noise.clock_quarter_frame();
                    }
                    FrameType::Half => {
                        self.pulse1.clock_half_frame();
                        self.pulse2.clock_half_frame();
                        self.triangle.clock_half_frame();
                        self.noise.clock_half_frame();
                    }
                    _ => (),
                });

            self.pulse1.length.reload();
            self.pulse2.length.reload();
            self.triangle.length.reload();
            self.noise.length.reload();

            self.pulse1
                .clock_to_output(self.cycle, &mut self.channel_outputs);
            self.pulse2
                .clock_to_output(self.cycle, &mut self.channel_outputs);
            self.triangle
                .clock_to_output(self.cycle, &mut self.channel_outputs);
            self.noise
                .clock_to_output(self.cycle, &mut self.channel_outputs);
            self.dmc
                .clock_to_output(self.cycle, &mut self.channel_outputs);
        }

        cycles
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl ApuRegisters for Apu {
    /// $4000 Pulse1, $4004 Pulse2, and $400C Noise Control.
    fn write_ctrl(&mut self, channel: Channel, val: u8) {
        self.clock_to(self.master_cycle);
        match channel {
            Channel::Pulse1 => self.pulse1.write_ctrl(val),
            Channel::Pulse2 => self.pulse2.write_ctrl(val),
            Channel::Noise => self.noise.write_ctrl(val),
            _ => panic!("{channel:?} does not have a control register"),
        }
        self.should_clock =
            self.pulse1.length.enabled || self.pulse2.length.enabled || self.noise.length.enabled;
    }

    /// $4001 Pulse1 and $4005 Pulse2 Sweep.
    fn write_sweep(&mut self, channel: Channel, val: u8) {
        self.clock_to(self.master_cycle);
        match channel {
            Channel::Pulse1 => self.pulse1.write_sweep(val),
            Channel::Pulse2 => self.pulse2.write_sweep(val),
            _ => panic!("{channel:?} does not have a sweep register"),
        }
    }

    /// $4002 Pulse1, $4006 Pulse2, $400A Triangle, $400E Noise, and $4010 DMC Timer Low Byte.
    fn write_timer_lo(&mut self, channel: Channel, val: u8) {
        self.clock_to(self.master_cycle);
        match channel {
            Channel::Pulse1 => self.pulse1.write_timer_lo(val),
            Channel::Pulse2 => self.pulse2.write_timer_lo(val),
            Channel::Triangle => self.triangle.write_timer_lo(val),
            Channel::Noise => self.noise.write_timer(val),
            Channel::Dmc => self.dmc.write_timer(val),
            _ => panic!("{channel:?} does not have a timer_lo register"),
        }
    }

    /// $4003 Pulse1, $4007 Pulse2, and $400B Triangle Timer High Byte.
    fn write_timer_hi(&mut self, channel: Channel, val: u8) {
        self.clock_to(self.master_cycle);
        match channel {
            Channel::Pulse1 => self.pulse1.write_timer_hi(val),
            Channel::Pulse2 => self.pulse2.write_timer_hi(val),
            Channel::Triangle => self.triangle.write_timer_hi(val),
            _ => panic!("{channel:?} does not have a timer_hi register"),
        }
        self.should_clock = self.pulse1.length.enabled
            || self.pulse2.length.enabled
            || self.triangle.length.enabled;
    }

    /// $4008 Triangle Linear Counter.
    fn write_linear_counter(&mut self, val: u8) {
        self.clock_to(self.master_cycle);
        self.triangle.write_linear_counter(val);
        self.should_clock = self.triangle.length.enabled;
    }

    /// $400F Noise and $4013 DMC Length.
    fn write_length(&mut self, channel: Channel, val: u8) {
        self.clock_to(self.master_cycle);
        match channel {
            Channel::Noise => self.noise.write_length(val),
            Channel::Dmc => self.dmc.write_length(val),
            _ => panic!("{channel:?} does not have a length register"),
        }
        self.should_clock = self.noise.length.enabled;
    }

    /// $4011 DMC Output Level.
    fn write_dmc_output(&mut self, val: u8) {
        self.clock_to(self.master_cycle);
        // Only 7-bits are used
        self.dmc
            .write_output_in(val & 0x7F, &mut self.channel_outputs);
    }

    /// $4012 DMC Sample Addr.
    fn write_dmc_addr(&mut self, val: u8) {
        self.clock_to(self.master_cycle);
        self.dmc.write_addr(val);
    }

    /// Read APU Status.
    ///
    /// $4015 | RW  | APU Status
    ///       |   0 | Channel 1, 1 = enable sound
    ///       |   1 | Channel 2, 1 = enable sound
    ///       |   2 | Channel 3, 1 = enable sound
    ///       |   3 | Channel 4, 1 = enable sound
    ///       |   4 | Channel 5, 1 = enable sound
    ///       | 5-7 | Unused (???)
    fn read_status(&mut self) -> u8 {
        self.clock_to(self.master_cycle);
        let val = self.peek_status();
        self.frame_counter.irq_pending = false;
        val
    }

    /// Read APU Status without side-effects.
    ///
    /// $4015 | RW  | APU Status
    ///       |   0 | Channel 1, 1 = enable sound
    ///       |   1 | Channel 2, 1 = enable sound
    ///       |   2 | Channel 3, 1 = enable sound
    ///       |   3 | Channel 4, 1 = enable sound
    ///       |   4 | Channel 5, 1 = enable sound
    ///       | 5-7 | Unused (???)
    ///
    /// Non-mutating version of `read_status`.
    fn peek_status(&self) -> u8 {
        let mut status = 0x00;
        if self.pulse1.length.counter > 0 {
            status |= 0x01;
        }
        if self.pulse2.length.counter > 0 {
            status |= 0x02;
        }
        if self.triangle.length.counter > 0 {
            status |= 0x04;
        }
        if self.noise.length.counter > 0 {
            status |= 0x08;
        }
        if self.dmc.bytes_remaining > 0 {
            status |= 0x10;
        }
        if self.frame_counter.irq_pending {
            status |= 0x40;
        }
        if self.dmc.irq_pending {
            status |= 0x80;
        }
        status
    }

    /// Write APU Status.
    ///
    /// $4015 | RW  | APU Status
    ///       |   0 | Channel 1, 1 = enable sound
    ///       |   1 | Channel 2, 1 = enable sound
    ///       |   2 | Channel 3, 1 = enable sound
    ///       |   3 | Channel 4, 1 = enable sound
    ///       |   4 | Channel 5, 1 = enable sound
    ///       | 5-7 | Unused (???)
    fn write_status(&mut self, val: u8) {
        self.clock_to(self.master_cycle);
        self.pulse1.set_enabled(val & 0x01 == 0x01);
        self.pulse2.set_enabled(val & 0x02 == 0x02);
        self.triangle.set_enabled(val & 0x04 == 0x04);
        self.noise.set_enabled(val & 0x08 == 0x08);
        self.dmc.set_enabled(val & 0x10 == 0x10, self.cpu_cycle);
    }

    /// $4017 APU Frame Counter.
    fn write_frame_counter(&mut self, val: u8) {
        self.clock_to(self.master_cycle);
        self.frame_counter.write(val, self.master_cycle);
    }
}

impl Regional for Apu {
    fn region(&self) -> NesRegion {
        self.region
    }

    fn set_region(&mut self, region: NesRegion) {
        if self.region != region {
            self.clock_to(self.master_cycle);
            self.region = region;
            self.clock_rate = Cpu::region_clock_rate(region);
            self.filter_chain = FilterChain::new(region, Self::SAMPLE_RATE);
            self.sample_period = self.clock_rate / Self::SAMPLE_RATE;
            self.frame_counter.set_region(region);
            self.noise.set_region(region);
            self.dmc.set_region(region);
        }
    }
}

impl Reset for Apu {
    fn reset(&mut self, kind: ResetKind) {
        self.master_cycle = 0;
        self.should_clock = false;
        self.frame_counter.reset(kind);
        self.pulse1.reset(kind);
        self.pulse2.reset(kind);
        self.triangle.reset(kind);
        self.noise.reset(kind);
        self.dmc.reset(kind);
    }
}

impl std::fmt::Debug for Apu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        f.debug_struct("Apu")
            .field("cycle", &self.master_cycle)
            .field("frame_counter", &self.frame_counter)
            .field("pulse1", &self.pulse1)
            .field("pulse2", &self.pulse2)
            .field("triangle", &self.triangle)
            .field("noise", &self.noise)
            .field("dmc", &self.dmc)
            .field("filter_chain", &self.filter_chain)
            .field("audio_samples_len", &self.audio_samples.len())
            .finish()
    }
}

// Generated values to avoid constant Lazy deref cost during runtime.
//
// Original calculation:
// let mut pulse_table = [0.0; 31];
// for (i, val) in pulse_table.iter_mut().enumerate().skip(1) {
//     *val = 95.52 / (8_128.0 / (i as f32) + 100.0);
// }
#[rustfmt::skip]
pub(crate) static PULSE_TABLE: [f32; 31] = [
    0.0,          0.011_609_139, 0.022_939_48, 0.034_000_948, 0.044_803,    0.055_354_66,
    0.065_664_53, 0.075_740_82,  0.085_591_4,  0.095_223_75,  0.104_645_04, 0.113_862_15,
    0.122_881_64, 0.131_709_8,   0.140_352_64, 0.148_815_96,  0.157_105_25, 0.165_225_88,
    0.173_182_92, 0.180_981_26,  0.188_625_59, 0.196_120_46,  0.203_470_17, 0.210_678_94,
    0.217_750_76, 0.224_689_5,   0.231_498_87, 0.238_182_47,  0.244_743_78, 0.251_186_07,
    0.257_512_57,
];

// Generated values to avoid constant Lazy deref cost during runtime.
//
// Original calculation:
// let mut tnd_table = [0.0; 203];
// for (i, val) in tnd_table.iter_mut().enumerate().skip(1) {
//     *val = 163.67 / (24_329.0 / (i as f32) + 100.0);
// }
#[rustfmt::skip]
static TND_TABLE: [f32; 203] = [
    0.0,           0.006_699_824, 0.013_345_02,  0.019_936_256, 0.026_474_18,  0.032_959_443,
    0.039_392_676, 0.045_774_5,   0.052_105_535, 0.058_386_38,  0.064_617_634, 0.070_799_87,
    0.076_933_69,  0.083_019_62,  0.089_058_26,  0.095_050_134, 0.100_995_794, 0.106_895_77,
    0.112_750_58,  0.118_560_754, 0.124_326_79,  0.130_049_18,  0.135_728_45,  0.141_365_05,
    0.146_959_5,   0.152_512_22,  0.158_023_7,   0.163_494_4,   0.168_924_76,  0.174_315_24,
    0.179_666_28,  0.184_978_3,   0.190_251_74,  0.195_486_98,  0.200_684_47,  0.205_844_63,
    0.210_967_81,  0.216_054_44,  0.221_104_92,  0.226_119_6,   0.231_098_88,  0.236_043_11,
    0.240_952_72,  0.245_828_,    0.250_669_36,  0.255_477_1,   0.260_251_64,  0.264_993_28,
    0.269_702_37,  0.274_379_22,  0.279_024_18,  0.283_637_58,  0.288_219_72,  0.292_770_95,
    0.297_291_52,  0.301_781_8,   0.306_242_1,   0.310_672_67,  0.315_073_85,  0.319_445_88,
    0.323_789_12,  0.328_103_78,  0.332_390_2,   0.336_648_6,   0.340_879_3,   0.345_082_55,
    0.349_258_63,  0.353_407_77,  0.357_530_27,  0.361_626_36,  0.365_696_34,  0.369_740_37,
    0.373_758_76,  0.377_751_74,  0.381_719_56,  0.385_662_44,  0.389_580_64,  0.393_474_37,
    0.397_343_84,  0.401_189_3,   0.405_011_,    0.408_809_07,  0.412_583_83,  0.416_335_46,
    0.420_064_15,  0.423_770_13,  0.427_453_6,   0.431_114_76,  0.434_753_84,  0.438_370_97,
    0.441_966_44,  0.445_540_4,   0.449_093_,    0.452_624_53,  0.456_135_06,  0.459_624_9,
    0.463_094_12,  0.466_542_93,  0.469_971_57,  0.473_380_15,  0.476_768_94,  0.480_137_94,
    0.483_487_52,  0.486_817_7,   0.490_128_73,  0.493_420_7,   0.496_693_88,  0.499_948_32,
    0.503_184_26,  0.506_401_84,  0.509_601_2,   0.512_782_45,  0.515_945_85,  0.519_091_4,
    0.522_219_5,   0.525_330_07,  0.528_423_25,  0.531_499_3,   0.534_558_36,  0.537_600_5,
    0.540_625_93,  0.543_634_8,   0.546_627_04,  0.549_603_04,  0.552_562_83,  0.555_506_47,
    0.558_434_3,   0.561_346_23,  0.564_242_5,   0.567_123_23,  0.569_988_5,   0.572_838_4,
    0.575_673_2,   0.578_492_94,  0.581_297_7,   0.584_087_6,   0.586_862_8,   0.589_623_45,
    0.592_369_56,  0.595_101_36,  0.597_818_9,   0.600_522_3,   0.603_211_6,   0.605_887_,
    0.608_548_64,  0.611_196_6,   0.613_830_8,   0.616_451_56,  0.619_059_,    0.621_653_14,
    0.624_234_,    0.626_801_85,  0.629_356_7,   0.631_898_64,  0.634_427_7,   0.636_944_2,
    0.639_448_05,  0.641_939_34,  0.644_418_24,  0.646_884_86,  0.649_339_2,   0.651_781_4,
    0.654_211_5,   0.656_629_74,  0.659_036_04,  0.661_430_6,   0.663_813_4,   0.666_184_66,
    0.668_544_35,  0.670_892_6,   0.673_229_46,  0.675_555_05,  0.677_869_44,  0.680_172_74,
    0.682_464_96,  0.684_746_2,   0.687_016_6,   0.689_276_2,   0.691_525_04,  0.693_763_3,
    0.695_990_9,   0.698_208_03,  0.700_414_8,   0.702_611_1,   0.704_797_2,   0.706_973_1,
    0.709_138_8,   0.711_294_5,   0.713_440_1,   0.715_575_9,   0.717_701_8,   0.719_817_9,
    0.721_924_25,  0.724_020_96,  0.726_108_,    0.728_185_65,  0.730_253_8,   0.732_312_56,
    0.734_361_95,  0.736_402_1,   0.738_433_1,   0.740_454_9,   0.742_467_6,
];
