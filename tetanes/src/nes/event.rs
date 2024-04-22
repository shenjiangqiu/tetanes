use crate::{
    nes::{
        action::{Action, Debug, DebugStep, Feature, Setting, Ui},
        config::Config,
        input::{Input, InputBindings},
        renderer::gui::Menu,
        Nes,
    },
    platform::{self, open_file_dialog},
};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tetanes_core::{
    action::Action as DeckAction,
    apu::Channel,
    common::{NesRegion, ResetKind},
    genie::GenieCode,
    input::{FourPlayer, JoypadBtn, Player},
    mem::RamState,
    time::Duration,
    video::VideoFilter,
};
use tracing::{error, trace};
use winit::{
    event::{ElementState, Event, Modifiers, WindowEvent},
    event_loop::{ControlFlow, EventLoopWindowTarget},
    keyboard::PhysicalKey,
    window::Fullscreen,
};

#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub enum UiEvent {
    Error(String),
    Message(String),
    RequestRedraw,
    PendingKeybind(bool),
    LoadRomDialog,
    LoadReplayDialog,
    Terminate,
}

#[derive(Clone, PartialEq)]
pub struct RomData(pub Vec<u8>);

impl std::fmt::Debug for RomData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RomData({} bytes)", self.0.len())
    }
}

impl AsRef<[u8]> for RomData {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, PartialEq)]
pub struct ReplayData(pub Vec<u8>);

impl std::fmt::Debug for ReplayData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReplayData({} bytes)", self.0.len())
    }
}

impl AsRef<[u8]> for ReplayData {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub enum ConfigEvent {
    ApuChannelEnabled((Channel, bool)),
    AudioBuffer(usize),
    AudioEnabled(bool),
    AudioLatency(Duration),
    AutoLoad(bool),
    AutoSave(bool),
    ConcurrentDpad(bool),
    CycleAccurate(bool),
    FourPlayer(FourPlayer),
    GenieCodeAdded(GenieCode),
    GenieCodeRemoved(String),
    InputBindings,
    RamState(RamState),
    Region(NesRegion),
    RewindEnabled(bool),
    RunAhead(usize),
    SaveSlot(u8),
    Speed(f32),
    VideoFilter(VideoFilter),
    Vsync(bool),
    ZapperConnected(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[must_use]
pub enum EmulationEvent {
    AudioRecord(bool),
    DebugStep(DebugStep),
    InstantRewind,
    Joypad((Player, JoypadBtn, ElementState)),
    #[serde(skip)]
    LoadReplay((String, ReplayData)),
    LoadReplayPath(PathBuf),
    #[serde(skip)]
    LoadRom((String, RomData)),
    LoadRomPath(PathBuf),
    LoadState(u8),
    Pause(bool),
    ReplayRecord(bool),
    Reset(ResetKind),
    Rewinding(bool),
    SaveState(u8),
    Screenshot,
    ZapperAim((u32, u32)),
    ZapperTrigger,
}

#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub enum RendererEvent {
    Frame,
    ScaleChanged,
    RomLoaded((String, NesRegion)),
    Menu(Menu),
}

#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub enum NesEvent {
    Ui(UiEvent),
    Emulation(EmulationEvent),
    Renderer(RendererEvent),
    Config(ConfigEvent),
}

impl From<UiEvent> for NesEvent {
    fn from(event: UiEvent) -> Self {
        Self::Ui(event)
    }
}

impl From<EmulationEvent> for NesEvent {
    fn from(event: EmulationEvent) -> Self {
        Self::Emulation(event)
    }
}

impl From<RendererEvent> for NesEvent {
    fn from(event: RendererEvent) -> Self {
        Self::Renderer(event)
    }
}

impl From<ConfigEvent> for NesEvent {
    fn from(event: ConfigEvent) -> Self {
        Self::Config(event)
    }
}

#[derive(Debug)]
#[must_use]
pub struct State {
    pub input_bindings: InputBindings,
    pub pending_keybind: bool,
    pub modifiers: Modifiers,
    pub occluded: bool,
    pub paused: bool,
    pub replay_recording: bool,
    pub audio_recording: bool,
    pub rewinding: bool,
    pub quitting: bool,
}

impl State {
    pub fn new(cfg: &Config) -> Self {
        Self {
            input_bindings: InputBindings::from_action_bindings(&cfg.input.bindings),
            pending_keybind: false,
            modifiers: Modifiers::default(),
            occluded: false,
            paused: false,
            replay_recording: false,
            audio_recording: false,
            rewinding: false,
            quitting: false,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}

impl Nes {
    pub fn event_loop(
        &mut self,
        event: Event<NesEvent>,
        event_loop: &EventLoopWindowTarget<NesEvent>,
    ) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        if self.state.quitting {
            event_loop.exit();
        } else if self.state.occluded {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
        }

        match event {
            Event::WindowEvent {
                window_id, event, ..
            } => {
                self.renderer.on_window_event(&self.window, &event);

                match event {
                    WindowEvent::CloseRequested => {
                        if window_id == self.window.id() {
                            event_loop.exit();
                        }
                    }
                    WindowEvent::RedrawRequested => {
                        if !self.state.occluded {
                            if let Err(err) =
                                self.renderer.request_redraw(&self.window, &mut self.cfg)
                            {
                                self.on_error(err);
                            }
                            self.window.request_redraw();
                        }
                    }
                    WindowEvent::Occluded(occluded) => {
                        if window_id == self.window.id() {
                            self.state.occluded = occluded;
                            // Don't unpause if paused manually
                            if !self.state.paused {
                                self.trigger_event(EmulationEvent::Pause(self.state.occluded));
                                self.window.request_redraw();
                            }
                        }
                    }
                    WindowEvent::KeyboardInput { event, .. } if !self.state.pending_keybind => {
                        if let PhysicalKey::Code(key) = event.physical_key {
                            self.on_input(
                                Input::Key(key, self.state.modifiers.state()),
                                event.state,
                                event.repeat,
                            );
                        }
                    }
                    WindowEvent::ModifiersChanged(modifiers) if !self.state.pending_keybind => {
                        self.state.modifiers = modifiers
                    }
                    WindowEvent::MouseInput { button, state, .. }
                        if !self.state.pending_keybind =>
                    {
                        self.on_input(Input::Mouse(button), state, false);
                    }
                    WindowEvent::DroppedFile(path) => {
                        self.trigger_event(EmulationEvent::LoadRomPath(path));
                    }
                    WindowEvent::HoveredFile(_) => (), // TODO: Show file drop cursor
                    WindowEvent::HoveredFileCancelled => (), // TODO: Restore cursor
                    _ => (),
                }
            }
            Event::AboutToWait => self.next_frame(),
            Event::LoopExiting => {
                #[cfg(feature = "profiling")]
                puffin::set_scopes_on(false);

                // Save window scale on exit
                let size = self.window.inner_size();
                let scale_factor = self.window.scale_factor() as f32;
                let texture_size = self.cfg.texture_size();
                let scale = if size.width < size.height {
                    (size.width as f32 / scale_factor) / texture_size.width as f32
                } else {
                    (size.height as f32 / scale_factor) / texture_size.height as f32
                };
                self.cfg.renderer.scale = scale.floor();
                if let Err(err) = self.cfg.save() {
                    error!("{err:?}");
                }
            }
            Event::UserEvent(event) => {
                self.emulation.on_event(&event);
                self.renderer.on_event(&event);
                if let NesEvent::Config(ConfigEvent::InputBindings) = event {
                    self.state.input_bindings =
                        InputBindings::from_action_bindings(&self.cfg.input.bindings);
                }
                if let NesEvent::Ui(event) = event {
                    self.on_event(event);
                }
            }
            _ => (),
        }
    }

    pub fn on_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Message(msg) => self.add_message(msg),
            UiEvent::Error(err) => self.on_error(anyhow!(err)),
            UiEvent::Terminate => self.state.quitting = true,
            UiEvent::RequestRedraw => self.window.request_redraw(),
            UiEvent::PendingKeybind(pending) => self.state.pending_keybind = pending,
            UiEvent::LoadRomDialog => {
                if platform::supports(platform::Feature::Filesystem) {
                    match open_file_dialog(
                        "Load ROM",
                        "NES ROMs",
                        &["nes"],
                        self.cfg
                            .renderer
                            .roms_path
                            .as_ref()
                            .map(|p| p.to_path_buf()),
                    ) {
                        Ok(maybe_path) => {
                            if let Some(path) = maybe_path {
                                self.trigger_event(EmulationEvent::LoadRomPath(path));
                            }
                        }
                        Err(err) => {
                            error!("failed top open rom dialog: {err:?}");
                            self.trigger_event(UiEvent::Error(
                                "failed to open rom dialog".to_string(),
                            ));
                        }
                    }
                }
            }
            UiEvent::LoadReplayDialog => {
                if platform::supports(platform::Feature::Filesystem) && self.renderer.rom_loaded() {
                    match open_file_dialog(
                        "Load Replay",
                        "Replay Recording",
                        &["replay"],
                        Config::default_data_dir(),
                    ) {
                        Ok(maybe_path) => {
                            if let Some(path) = maybe_path {
                                self.trigger_event(EmulationEvent::LoadReplayPath(path));
                            }
                        }
                        Err(err) => {
                            error!("failed top open replay dialog: {err:?}");
                            self.trigger_event(UiEvent::Error(
                                "failed to open replay dialog".to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Trigger a custom event.
    pub fn trigger_event(&mut self, event: impl Into<NesEvent>) {
        let event = event.into();
        trace!("Nes event: {event:?}");

        self.emulation.on_event(&event);
        self.renderer.on_event(&event);
        match event {
            NesEvent::Ui(event) => self.on_event(event),
            NesEvent::Emulation(EmulationEvent::LoadRomPath(path)) => {
                if let Ok(path) = path.canonicalize() {
                    self.cfg.renderer.recent_roms.insert(path);
                }
            }
            _ => (),
        }
    }

    /// Handle user input mapped to key bindings.
    pub fn on_input(&mut self, input: Input, state: ElementState, repeat: bool) {
        if let Some((action, player)) = self.state.input_bindings.get(&input).copied() {
            trace!("player: {player:?}, action: {action:?}, state: {state:?}, repeat: {repeat:?}");
            let released = state == ElementState::Released;
            match action {
                Action::Ui(state) if released => match state {
                    Ui::Quit => self.trigger_event(UiEvent::Terminate),
                    Ui::TogglePause => {
                        self.state.paused = !self.state.paused;
                        self.trigger_event(EmulationEvent::Pause(self.state.paused));
                    }
                    Ui::LoadRom => {
                        self.state.paused = !self.state.paused;
                        self.trigger_event(EmulationEvent::Pause(self.state.paused));
                        self.trigger_event(UiEvent::LoadRomDialog);
                    }
                    Ui::LoadReplay => {
                        self.state.paused = !self.state.paused;
                        self.trigger_event(EmulationEvent::Pause(self.state.paused));
                        self.trigger_event(UiEvent::LoadReplayDialog);
                    }
                },
                Action::Menu(menu) if released => self.trigger_event(RendererEvent::Menu(menu)),
                Action::Feature(feature) => match feature {
                    Feature::ToggleReplayRecording if released => {
                        if platform::supports(platform::Feature::Filesystem) {
                            self.state.replay_recording = !self.state.replay_recording;
                            self.trigger_event(EmulationEvent::ReplayRecord(
                                self.state.replay_recording,
                            ));
                        } else {
                            self.add_message(
                                "replay recordings are not supported yet on this platform.",
                            );
                        }
                    }
                    Feature::ToggleAudioRecording if released => {
                        if platform::supports(platform::Feature::Filesystem) {
                            self.state.audio_recording = !self.state.audio_recording;
                            self.trigger_event(EmulationEvent::AudioRecord(
                                self.state.audio_recording,
                            ));
                        } else {
                            self.add_message(
                                "audio recordings are not supported yet on this platform.",
                            );
                        }
                    }
                    Feature::TakeScreenshot if released => {
                        if platform::supports(platform::Feature::Filesystem) {
                            self.trigger_event(EmulationEvent::Screenshot);
                        } else {
                            self.add_message("screenshots are not supported yet on this platform.");
                        }
                    }
                    Feature::VisualRewind => {
                        if !self.state.rewinding {
                            if repeat {
                                self.state.rewinding = true;
                                self.trigger_event(EmulationEvent::Rewinding(self.state.rewinding));
                            } else if released {
                                self.trigger_event(EmulationEvent::InstantRewind);
                            }
                        } else if released {
                            self.state.rewinding = false;
                            self.trigger_event(EmulationEvent::Rewinding(self.state.rewinding));
                        }
                    }
                    _ => (),
                },
                Action::Setting(setting) => match setting {
                    Setting::ToggleFullscreen if released => {
                        self.cfg.renderer.fullscreen = !self.cfg.renderer.fullscreen;
                        self.window.set_fullscreen(
                            self.cfg
                                .renderer
                                .fullscreen
                                .then_some(Fullscreen::Borderless(None)),
                        );
                    }
                    Setting::ToggleVsync if released => {
                        if platform::supports(platform::Feature::ToggleVsync) {
                            self.cfg.renderer.vsync = !self.cfg.renderer.vsync;
                            self.trigger_event(ConfigEvent::Vsync(self.cfg.renderer.vsync));
                        } else {
                            self.add_message("Disabling VSync is not supported on this platform.");
                        }
                    }
                    Setting::ToggleAudio if released => {
                        self.cfg.audio.enabled = !self.cfg.audio.enabled;
                        self.trigger_event(ConfigEvent::AudioEnabled(self.cfg.audio.enabled));
                    }
                    Setting::ToggleMenubar if released => {
                        self.cfg.renderer.show_menubar = !self.cfg.renderer.show_menubar;
                    }
                    Setting::IncrementScale if released => {
                        let scale = self.cfg.renderer.scale;
                        let new_scale = self.cfg.increment_scale();
                        if scale != new_scale {
                            self.trigger_event(RendererEvent::ScaleChanged);
                        }
                    }
                    Setting::DecrementScale if released => {
                        let scale = self.cfg.renderer.scale;
                        let new_scale = self.cfg.decrement_scale();
                        if scale != new_scale {
                            self.trigger_event(RendererEvent::ScaleChanged);
                        }
                    }
                    Setting::IncrementSpeed if released => {
                        let speed = self.cfg.emulation.speed;
                        let new_speed = self.cfg.increment_speed();
                        if speed != new_speed {
                            self.trigger_event(ConfigEvent::Speed(self.cfg.emulation.speed));
                            self.add_message(format!("Increased Emulation Speed to {new_speed}"));
                        }
                    }
                    Setting::DecrementSpeed if released => {
                        let speed = self.cfg.emulation.speed;
                        let new_speed = self.cfg.decrement_speed();
                        if speed != new_speed {
                            self.trigger_event(ConfigEvent::Speed(self.cfg.emulation.speed));
                            self.add_message(format!("Decreased Emulation Speed to {new_speed}"));
                        }
                    }
                    Setting::FastForward if !repeat => {
                        let new_speed = if released { 1.0 } else { 2.0 };
                        let speed = self.cfg.emulation.speed;
                        if speed != new_speed {
                            self.cfg.emulation.speed = new_speed;
                            self.trigger_event(ConfigEvent::Speed(self.cfg.emulation.speed));
                            if new_speed == 2.0 {
                                self.add_message("Fast forwarding");
                            }
                        }
                    }
                    _ => (),
                },
                Action::Deck(action) => match action {
                    DeckAction::Reset(kind) if released => {
                        self.trigger_event(EmulationEvent::Reset(kind));
                    }
                    DeckAction::Joypad(button) if !repeat => {
                        if let Some(player) = player {
                            self.trigger_event(EmulationEvent::Joypad((player, button, state)));
                        }
                    }
                    DeckAction::ZapperConnect(connected) => {
                        self.cfg.deck.zapper = connected;
                        self.trigger_event(ConfigEvent::ZapperConnected(self.cfg.deck.zapper));
                    }
                    DeckAction::ZapperAim((x, y)) => {
                        self.trigger_event(EmulationEvent::ZapperAim((x, y)));
                    }
                    DeckAction::ZapperTrigger => {
                        if self.cfg.deck.zapper {
                            self.trigger_event(EmulationEvent::ZapperTrigger);
                        }
                    }
                    DeckAction::SetSaveSlot(slot) if released => {
                        if platform::supports(platform::Feature::Filesystem) {
                            if self.cfg.emulation.save_slot != slot {
                                self.cfg.emulation.save_slot = slot;
                                self.add_message(format!("Changed Save Slot to {slot}"));
                            }
                        } else {
                            self.add_message("save states are not supported yet on this platform.");
                        }
                    }
                    DeckAction::SaveState if released => {
                        if platform::supports(platform::Feature::Filesystem) {
                            self.trigger_event(EmulationEvent::SaveState(
                                self.cfg.emulation.save_slot,
                            ));
                        } else {
                            self.add_message("save states are not supported yet on this platform.");
                        }
                    }
                    DeckAction::LoadState if released => {
                        if platform::supports(platform::Feature::Filesystem) {
                            self.trigger_event(EmulationEvent::LoadState(
                                self.cfg.emulation.save_slot,
                            ));
                        } else {
                            self.add_message("save states are not supported yet on this platform.");
                        }
                    }
                    DeckAction::ToggleApuChannel(channel) if released => {
                        self.cfg.deck.channels_enabled[channel as usize] =
                            !self.cfg.deck.channels_enabled[channel as usize];
                        self.trigger_event(ConfigEvent::ApuChannelEnabled((
                            channel,
                            self.cfg.deck.channels_enabled[channel as usize],
                        )));
                    }
                    DeckAction::MapperRevision(_) if released => todo!("mapper revision"),
                    DeckAction::SetNesRegion(region) if released => {
                        self.cfg.deck.region = region;
                        self.trigger_event(ConfigEvent::Region(self.cfg.deck.region));
                        self.add_message(format!("Changed NES Region to {region:?}"));
                    }
                    DeckAction::SetVideoFilter(filter) if released => {
                        let filter = if self.cfg.deck.filter == filter {
                            VideoFilter::Pixellate
                        } else {
                            filter
                        };
                        self.trigger_event(ConfigEvent::VideoFilter(filter));
                    }
                    _ => (),
                },
                Action::Debug(action) => match action {
                    Debug::Toggle(kind) if released => {
                        self.add_message(format!("{kind:?} is not implemented yet"));
                    }
                    Debug::Step(step) if released | repeat => {
                        self.trigger_event(EmulationEvent::DebugStep(step));
                    }
                    _ => (),
                },
                _ => (),
            }
        }
    }
}
