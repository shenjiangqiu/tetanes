use crate::nes::{
    action::DebugStep,
    audio::{Audio, State as AudioState},
    config::{Config, FrameRate},
    emulation::{replay::Record, rewind::Rewind},
    event::{ConfigEvent, EmulationEvent, NesEvent, RendererEvent, SendNesEvent, UiEvent},
    renderer::BufferPool,
};
use anyhow::{anyhow, bail};
use chrono::Local;
use crossbeam::channel::{self, Receiver, Sender};
use replay::Replay;
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    thread::JoinHandle,
};
use tetanes_core::{
    apu::Apu,
    common::{NesRegion, Regional, Reset, ResetKind},
    control_deck::{self, ControlDeck},
    cpu::Cpu,
    fs,
    ppu::Ppu,
    time::{Duration, Instant},
};
use tracing::{debug, error, trace};
use winit::{event::ElementState, event_loop::EventLoopProxy};

pub mod replay;
pub mod rewind;

#[derive(Debug)]
#[must_use]
enum Threads {
    Single(Single),
    Multi(Multi),
}

#[derive(Debug)]
#[must_use]
struct Single {
    state: State,
}

#[derive(Debug)]
#[must_use]
struct Multi {
    tx: Sender<NesEvent>,
    handle: JoinHandle<()>,
}

impl Multi {
    fn spawn(
        proxy_tx: EventLoopProxy<NesEvent>,
        frame_pool: BufferPool,
        config: Config,
    ) -> anyhow::Result<Self> {
        let (tx, rx) = channel::bounded(1024);
        Ok(Self {
            tx,
            handle: std::thread::Builder::new()
                .name("emulation".into())
                .spawn(move || Self::main(proxy_tx, rx, frame_pool, config))?,
        })
    }

    fn main(
        tx: EventLoopProxy<NesEvent>,
        rx: Receiver<NesEvent>,
        frame_pool: BufferPool,
        config: Config,
    ) {
        debug!("emulation thread started");
        let mut state = State::new(tx, frame_pool, config); // Has to be created on the thread, since
        loop {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("emulation loop");

            while let Ok(event) = rx.try_recv() {
                state.on_event(&event);
            }

            state.clock();
        }
    }
}

#[derive(Debug)]
#[must_use]
pub struct Emulation {
    threads: Threads,
}

impl Emulation {
    /// Initializes the renderer in a platform-agnostic way.
    pub fn initialize(
        tx: EventLoopProxy<NesEvent>,
        frame_pool: BufferPool,
        cfg: Config,
    ) -> anyhow::Result<Self> {
        let threaded = cfg.emulation.threaded
            && std::thread::available_parallelism().map_or(false, |count| count.get() > 1);
        let backend = if threaded {
            Threads::Multi(Multi::spawn(tx, frame_pool, cfg)?)
        } else {
            Threads::Single(Single {
                state: State::new(tx, frame_pool, cfg),
            })
        };

        Ok(Self { threads: backend })
    }

    /// Handle event.
    pub fn on_event(&mut self, event: &NesEvent) {
        match &mut self.threads {
            Threads::Single(Single { state }) => state.on_event(event),
            Threads::Multi(Multi { tx, handle }) => {
                handle.thread().unpark();
                if let Err(err) = tx.try_send(event.clone()) {
                    error!("failed to send event to emulation thread: {event:?}. {err:?}");
                    std::process::exit(1);
                }
            }
        }
    }

    pub fn request_clock_frame(&mut self) -> anyhow::Result<()> {
        // Multi-threaded emulation will handle frame clocking on its own
        if let Threads::Single(Single { ref mut state }) = self.threads {
            state.clock();
        }
        Ok(())
    }
}

#[derive(Debug)]
#[must_use]
pub struct State {
    tx: EventLoopProxy<NesEvent>,
    control_deck: ControlDeck,
    audio: Audio,
    frame_pool: BufferPool,
    frame_latency: usize,
    target_frame_duration: Duration,
    last_frame_time: Instant,
    total_frame_duration: Duration,
    frame_time_accumulator: f32,
    occluded: bool,
    paused: bool,
    rewinding: bool,
    rewind: Rewind,
    record: Record,
    replay: Replay,
    save_slot: u8,
    auto_save: bool,
    auto_load: bool,
    speed: f32,
    run_ahead: usize,
}

impl Drop for State {
    fn drop(&mut self) {
        self.unload_rom();
    }
}

impl State {
    pub fn new(tx: EventLoopProxy<NesEvent>, frame_pool: BufferPool, cfg: Config) -> Self {
        let control_deck = ControlDeck::with_config(cfg.deck.clone());
        let audio = Audio::new(
            cfg.audio.enabled,
            Apu::SAMPLE_RATE * cfg.emulation.speed,
            cfg.audio.latency,
            cfg.audio.buffer_size,
        );
        let rewind = Rewind::new(cfg.emulation.rewind);
        let mut state = Self {
            tx,
            control_deck,
            audio,
            frame_pool,
            frame_latency: 1,
            target_frame_duration: FrameRate::from(cfg.deck.region).duration(),
            last_frame_time: Instant::now(),
            total_frame_duration: Duration::default(),
            frame_time_accumulator: 0.0,
            occluded: false,
            paused: true,
            rewinding: false,
            rewind,
            record: Record::new(),
            replay: Replay::new(),
            save_slot: cfg.emulation.save_slot,
            auto_save: cfg.emulation.auto_save,
            auto_load: cfg.emulation.auto_load,
            speed: cfg.emulation.speed,
            run_ahead: cfg.emulation.run_ahead,
        };
        state.update_region(cfg.deck.region);
        state
    }

    pub fn add_message<S: ToString>(&mut self, msg: S) {
        self.tx.nes_event(UiEvent::Message(msg.to_string()));
    }

    pub fn write_deck<T>(
        &mut self,
        writer: impl FnOnce(&mut ControlDeck) -> control_deck::Result<T>,
    ) -> Option<T> {
        writer(&mut self.control_deck)
            .map_err(|err| {
                self.pause(true);
                self.on_error(err);
            })
            .ok()
    }

    pub fn on_error(&mut self, err: impl Into<anyhow::Error>) {
        let err = err.into();
        error!("Emulation error: {err:?}");
        self.add_message(err);
    }

    /// Handle event.
    pub fn on_event(&mut self, event: &NesEvent) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        match event {
            NesEvent::Emulation(event) => self.on_emulation_event(event),
            NesEvent::Config(event) => self.on_config_event(event),
            _ => (),
        }
    }

    /// Handle emulation event.
    pub fn on_emulation_event(&mut self, event: &EmulationEvent) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        match event {
            EmulationEvent::AudioRecord(recording) => {
                if self.control_deck.is_running() {
                    self.audio_record(*recording);
                }
            }
            EmulationEvent::DebugStep(step) => {
                if self.control_deck.is_running() {
                    match step {
                        DebugStep::Into => {
                            self.write_deck(|deck| deck.clock_instr());
                        }
                        DebugStep::Out => {
                            // TODO: track stack frames list on jsr, irq, brk
                            // while stack frame == previous stack frame, clock_instr, send_frame
                        }
                        DebugStep::Over => {
                            // TODO: track stack frames list on jsr, irq, brk
                            // while stack frame != previous stack frame, clock_instr, send_frame
                        }
                        DebugStep::Scanline => {
                            if self.write_deck(|deck| deck.clock_scanline()).is_some() {
                                self.send_frame();
                            }
                        }
                        DebugStep::Frame => {
                            if self.write_deck(|deck| deck.clock_frame()).is_some() {
                                self.send_frame();
                            }
                        }
                    }
                }
            }
            EmulationEvent::InstantRewind => {
                if self.control_deck.is_running() {
                    self.instant_rewind();
                }
            }
            EmulationEvent::Joypad((player, button, state)) => {
                if self.control_deck.is_running() {
                    let pressed = *state == ElementState::Pressed;
                    let joypad = self.control_deck.joypad_mut(*player);
                    joypad.set_button(*button, pressed);
                    self.record
                        .push(self.control_deck.frame_number(), event.clone());
                }
            }
            EmulationEvent::LoadReplay((name, replay)) => {
                if self.control_deck.is_running() {
                    self.load_replay(name, &mut io::Cursor::new(replay));
                }
            }
            EmulationEvent::LoadReplayPath(path) => {
                if self.control_deck.is_running() {
                    self.load_replay_path(path);
                }
            }
            EmulationEvent::LoadRom((name, rom)) => {
                self.load_rom(name, &mut io::Cursor::new(rom));
            }
            EmulationEvent::LoadRomPath(path) => self.load_rom_path(path),
            EmulationEvent::LoadState(slot) => {
                if let Some(rom) = self.control_deck.loaded_rom() {
                    if let Some(path) = Config::save_path(rom, *slot) {
                        match self.control_deck.load_state(path) {
                            Ok(_) => self.add_message(format!("State {slot} Loaded")),
                            Err(err) => self.on_error(err),
                        }
                    }
                }
            }
            EmulationEvent::Pause(paused) => {
                if self.control_deck.is_running() {
                    self.pause(*paused);
                }
            }
            EmulationEvent::ReplayRecord(recording) => {
                if self.control_deck.is_running() {
                    self.replay_record(*recording);
                }
            }
            EmulationEvent::Reset(kind) => {
                if self.control_deck.is_running() {
                    self.control_deck.reset(*kind);
                    self.pause(false);
                    match kind {
                        ResetKind::Soft => self.add_message("Reset"),
                        ResetKind::Hard => self.add_message("Power Cycled"),
                    }
                }
            }
            EmulationEvent::Rewinding(rewind) => {
                if self.control_deck.is_running() {
                    if self.rewind.enabled {
                        self.rewinding = *rewind;
                        self.add_message("Rewinding...");
                    } else {
                        self.rewind_disabled();
                    }
                }
            }
            EmulationEvent::SaveState(slot) => {
                if let Some(rom) = self.control_deck.loaded_rom() {
                    if let Some(data_dir) = Config::save_path(rom, *slot) {
                        match self.control_deck.save_state(data_dir) {
                            Ok(_) => self.add_message(format!("State {slot} Saved")),
                            Err(err) => self.on_error(err),
                        }
                    }
                }
            }
            EmulationEvent::Screenshot => {
                if self.control_deck.is_running() {
                    match self.save_screenshot() {
                        Ok(filename) => {
                            self.add_message(format!("Screenshot Saved: {}", filename.display()));
                        }
                        Err(err) => self.on_error(err),
                    }
                }
            }
            EmulationEvent::UnloadRom => self.unload_rom(),
            EmulationEvent::ZapperAim((x, y)) => {
                self.control_deck.aim_zapper(*x, *y);
                self.record
                    .push(self.control_deck.frame_number(), event.clone());
            }
            EmulationEvent::ZapperTrigger => {
                self.control_deck.trigger_zapper();
                self.record
                    .push(self.control_deck.frame_number(), event.clone());
            }
        }
    }

    /// Handle config event.
    pub fn on_config_event(&mut self, event: &ConfigEvent) {
        match event {
            ConfigEvent::ApuChannelEnabled((channel, enabled)) => {
                self.control_deck
                    .set_apu_channel_enabled(*channel, *enabled);
                let enabled_text = if *enabled { "Enabled" } else { "Disabled" };
                self.add_message(format!("{enabled_text} APU Channel {channel:?}"));
            }
            ConfigEvent::AudioBuffer(buffer_size) => {
                if let Err(err) = self.audio.set_buffer_size(*buffer_size) {
                    self.on_error(err);
                }
            }
            ConfigEvent::AudioEnabled(enabled) => match self.audio.set_enabled(*enabled) {
                Ok(state) => match state {
                    AudioState::Started => self.add_message("Audio Enabled"),
                    AudioState::Disabled | AudioState::Stopped => {
                        self.add_message("Audio Disabled")
                    }
                    AudioState::NoOutputDevice => (),
                },
                Err(err) => self.on_error(err),
            },
            ConfigEvent::AudioLatency(latency) => {
                if let Err(err) = self.audio.set_latency(*latency) {
                    self.on_error(err);
                }
            }
            ConfigEvent::AutoLoad(enabled) => self.auto_load = *enabled,
            ConfigEvent::AutoSave(enabled) => self.auto_save = *enabled,
            ConfigEvent::ConcurrentDpad(enabled) => {
                self.control_deck.set_concurrent_dpad(*enabled);
            }
            ConfigEvent::CycleAccurate(enabled) => {
                self.control_deck.set_cycle_accurate(*enabled);
            }
            ConfigEvent::FourPlayer(four_player) => {
                self.control_deck.set_four_player(*four_player);
            }
            ConfigEvent::GenieCodeAdded(genie_code) => {
                self.control_deck.cpu.bus.add_genie_code(genie_code.clone());
            }
            ConfigEvent::GenieCodeRemoved(code) => {
                self.control_deck.remove_genie_code(code);
            }
            ConfigEvent::RamState(ram_state) => {
                self.control_deck.set_ram_state(*ram_state);
            }
            ConfigEvent::Region(region) => {
                self.control_deck.set_region(*region);
                self.update_region(*region);
            }
            ConfigEvent::RewindEnabled(enabled) => {
                self.rewind.set_enabled(*enabled);
            }
            ConfigEvent::RunAhead(run_ahead) => self.run_ahead = *run_ahead,
            ConfigEvent::SaveSlot(slot) => self.save_slot = *slot,
            ConfigEvent::Speed(speed) => {
                self.speed = *speed;
                self.control_deck.set_frame_speed(*speed);
            }
            ConfigEvent::VideoFilter(filter) => self.control_deck.set_filter(*filter),
            ConfigEvent::ZapperConnected(connected) => {
                self.control_deck.connect_zapper(*connected);
            }
            ConfigEvent::Fullscreen(_)
            | ConfigEvent::HideOverscan(_)
            | ConfigEvent::InputBindings
            | ConfigEvent::Scale(_)
            | ConfigEvent::Vsync(_) => (),
        }
    }

    fn send_frame(&mut self) {
        self.tx.nes_event(RendererEvent::Frame);
        if let Ok(mut frame) = self.frame_pool.push_ref() {
            self.control_deck.frame_buffer_into(&mut frame);
            self.tx.nes_event(UiEvent::RequestRedraw);
        }
    }

    pub fn pause(&mut self, paused: bool) {
        if !self.control_deck.cpu_corrupted() {
            self.paused = paused;
            if self.paused {
                if let Some(rom) = self.control_deck.loaded_rom() {
                    if let Err(err) = self.record.stop(rom) {
                        self.on_error(err);
                    }
                }
            }
            self.audio.pause(self.paused);
        } else {
            self.paused = true;
        }
    }

    fn unload_rom(&mut self) {
        if let Some(rom) = self.control_deck.loaded_rom() {
            if self.auto_save {
                if let Some(path) = Config::save_path(rom, self.save_slot) {
                    if let Err(err) = self.control_deck.save_state(path) {
                        self.on_error(err);
                    }
                }
            }
            self.replay_record(false);
            let _ = self.audio.stop();
            if let Err(err) = self.control_deck.unload_rom() {
                self.on_error(err);
            }
            self.tx.nes_event(RendererEvent::RomUnloaded);
        }
    }

    fn on_load_rom(&mut self, name: impl Into<String>) {
        let name = name.into();
        if self.auto_load {
            if let Some(path) = Config::save_path(&name, self.save_slot) {
                if let Err(err) = self.control_deck.load_state(path) {
                    error!("failed to load state: {err:?}");
                }
            }
        }
        self.tx.nes_event(RendererEvent::RomLoaded((
            name,
            self.control_deck.cart_region,
        )));
        if let Err(err) = self.audio.start() {
            self.on_error(err);
        }
        self.pause(false);
    }

    fn load_rom_path(&mut self, path: impl AsRef<std::path::Path>) {
        let path = path.as_ref();
        self.unload_rom();
        match self.control_deck.load_rom_path(path) {
            Ok(()) => {
                let filename = fs::filename(path);
                self.on_load_rom(filename);
            }
            Err(err) => self.on_error(err),
        }
    }

    fn load_rom(&mut self, name: &str, rom: &mut impl Read) {
        self.unload_rom();
        match self.control_deck.load_rom(name, rom) {
            Ok(()) => self.on_load_rom(name),
            Err(err) => self.on_error(err),
        }
    }

    fn on_load_replay(&mut self, start: Cpu, name: impl AsRef<str>) {
        self.add_message(format!("Loaded Replay Recording {:?}", name.as_ref()));
        self.control_deck.load_cpu(start);
        self.pause(false);
    }

    fn load_replay_path(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        match self.replay.load_path(path) {
            Ok(start) => self.on_load_replay(start, path.to_string_lossy()),
            Err(err) => self.on_error(err),
        }
    }

    fn load_replay(&mut self, name: &str, replay: &mut impl Read) {
        match self.replay.load(replay) {
            Ok(start) => self.on_load_replay(start, name),
            Err(err) => self.on_error(err),
        }
    }

    fn update_region(&mut self, region: NesRegion) {
        self.target_frame_duration = FrameRate::from(region).duration();
        self.frame_latency = (self.audio.latency.as_secs_f32()
            / self.target_frame_duration.as_secs_f32())
        .ceil() as usize;
    }

    fn audio_record(&mut self, recording: bool) {
        if self.control_deck.is_running() {
            if !recording && self.audio.is_recording() {
                self.audio.set_recording(false);
                self.add_message("Audio Recording Stopped");
            } else if recording {
                self.audio.set_recording(true);
                self.add_message("Audio Recording Started");
            }
        }
    }

    fn replay_record(&mut self, recording: bool) {
        if self.control_deck.is_running() {
            if recording {
                self.record.start(self.control_deck.cpu().clone());
                self.add_message("Replay Recording Started");
            } else if let Some(rom) = self.control_deck.loaded_rom() {
                match self.record.stop(rom) {
                    Ok(Some(filename)) => {
                        self.add_message(format!("Saved Replay Recording {filename:?}"));
                    }
                    Err(err) => self.on_error(err),
                    _ => (),
                }
            }
        }
    }

    pub fn save_screenshot(&mut self) -> anyhow::Result<PathBuf> {
        match Config::default_picture_dir() {
            Some(picture_dir) => {
                let filename = picture_dir
                    .join(
                        Local::now()
                            .format("screenshot_%Y-%m-%d_at_%H_%M_%S")
                            .to_string(),
                    )
                    .with_extension("png");
                let image = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
                    Ppu::WIDTH,
                    Ppu::HEIGHT,
                    self.control_deck.frame_buffer(),
                )
                .ok_or_else(|| anyhow!("failed to create image buffer"))?;

                // TODO: provide wasm download
                Ok(image.save(&filename).map(|_| filename)?)
            }
            None => bail!("failed to find default picture directory"),
        }
    }

    fn sleep(&self) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        let timeout = if self.audio.enabled() {
            self.audio.queued_time().saturating_sub(self.audio.latency)
        } else {
            (self.last_frame_time + self.target_frame_duration)
                .saturating_duration_since(Instant::now())
        };
        let epsilon = Duration::from_millis(1);
        if timeout > epsilon {
            let timeout = timeout.min(self.target_frame_duration - epsilon);
            trace!("sleeping for {:.4}s", timeout.as_secs_f32());
            std::thread::park_timeout(timeout);
        }
    }

    fn clock(&mut self) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        if self.paused || self.occluded || !self.control_deck.is_running() {
            if self.paused && !self.occluded && self.control_deck.is_running() {
                self.send_frame();
            }
            return std::thread::park();
        }

        let last_frame_duration = self.last_frame_time.elapsed();
        trace!("last frame: {:.4}s", last_frame_duration.as_secs_f32());
        self.last_frame_time = Instant::now();
        self.total_frame_duration += last_frame_duration;
        self.frame_time_accumulator += last_frame_duration.as_secs_f32();
        if self.frame_time_accumulator > 0.025 {
            self.frame_time_accumulator = 0.025;
        }

        // Clock frames until we catch up to the audio queue latency as long as audio is enabled and we're
        // not rewinding, otherwise fall back to time-based clocking
        let mut clocked_frames = 0; // Prevent infinite loop when queued audio falls behind
        let frame_duration_seconds = self.target_frame_duration.as_secs_f32();
        let mut run_ahead = self.run_ahead;
        if self.speed > 1.0 {
            run_ahead = 0;
        }
        while if self.audio.enabled() && !self.rewinding {
            self.audio.queued_time() <= self.audio.latency && clocked_frames <= self.frame_latency
        } else {
            self.frame_time_accumulator >= frame_duration_seconds
        } {
            #[cfg(feature = "profiling")]
            puffin::profile_scope!("clock");

            if self.rewinding {
                match self.rewind.pop() {
                    Some(cpu) => self.control_deck.load_cpu(cpu),
                    None => self.rewinding = false,
                }
                self.send_frame();
            } else {
                if let Some(event) = self.replay.next(self.control_deck.frame_number()) {
                    self.on_emulation_event(&event);
                }
                let res = self.control_deck.clock_frame_ahead(
                    run_ahead,
                    |_cycles, frame_buffer, audio_samples| {
                        self.audio.process(audio_samples);
                        self.tx.nes_event(RendererEvent::Frame);
                        if let Ok(mut frame) = self.frame_pool.push_ref() {
                            frame.clear();
                            frame.extend_from_slice(frame_buffer);
                            self.tx.nes_event(UiEvent::RequestRedraw);
                        }
                    },
                );
                match res {
                    Ok(()) => {
                        if let Err(err) = self.rewind.push(self.control_deck.cpu()) {
                            self.rewind.set_enabled(false);
                            self.on_error(err);
                            break;
                        }
                    }
                    Err(err) => {
                        self.pause(true);
                        self.on_error(err);
                        break;
                    }
                }
            }

            self.frame_time_accumulator -= frame_duration_seconds;
            clocked_frames += 1;
        }

        self.sleep();
    }
}
