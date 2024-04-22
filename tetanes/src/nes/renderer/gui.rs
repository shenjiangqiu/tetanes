use crate::{
    nes::{
        action::{Action, Debug, DebugStep, Debugger, Feature, Setting, Ui as UiAction},
        config::Config,
        event::{ConfigEvent, EmulationEvent, NesEvent, UiEvent},
        input::{ActionBindings, Input},
    },
    platform,
};
use egui::{
    ahash::{HashMap, HashMapExt},
    global_dark_light_mode_switch,
    load::SizedTexture,
    menu, Align, Align2, Button, CentralPanel, Color32, Context, CursorIcon, Direction, DragValue,
    FontData, FontDefinitions, FontFamily, Frame, Grid, Image, Key, KeyboardShortcut, Layout,
    Modifiers, PointerButton, RichText, ScrollArea, Slider, TopBottomPanel, Ui, Vec2,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, mem, sync::Arc};
use tetanes_core::{
    action::Action as DeckAction,
    apu::Channel,
    common::{NesRegion, ResetKind},
    fs,
    genie::GenieCode,
    input::{FourPlayer, Player},
    mem::RamState,
    ppu::Ppu,
    time::{Duration, Instant},
    video::VideoFilter,
};
use tracing::{error, info, trace};
use winit::{
    event::MouseButton,
    event_loop::EventLoopProxy,
    keyboard::{KeyCode, ModifiersState},
    window::{Fullscreen, Window},
};

pub const MSG_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_MESSAGES: usize = 5;
pub const MENU_WIDTH: f32 = 200.0;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Menu {
    Preferences,
    Keybinds,
    About,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PreferencesTab {
    Emulation,
    Audio,
    Video,
    Input,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum KeybindsTab {
    Shortcuts,
    Joypad(Player),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetKeybind {
    action: Action,
    player: Option<Player>,
    binding: usize,
    input: Option<Input>,
    conflict: Option<(Action, Option<Player>)>,
}

type Keybind = (Action, [Option<Input>; 2]);

#[derive(Debug)]
#[must_use]
pub struct Gui {
    initialized: bool,
    window: Arc<Window>,
    pub(super) title: String,
    event_proxy: EventLoopProxy<NesEvent>,
    debounced_events: HashMap<&'static str, (NesEvent, Instant)>,
    pub(super) texture: SizedTexture,
    pub(super) paused: bool,
    pub(super) menu_height: f32,
    pub(super) preferences_open: bool,
    pub(super) keybinds_open: bool,
    pub(super) about_open: bool,
    preferences_tab: PreferencesTab,
    keybinds_tab: KeybindsTab,
    pending_keybind: Option<SetKeybind>,
    cpu_debugger_open: bool,
    ppu_debugger_open: bool,
    apu_debugger_open: bool,
    pub(super) cart_aspect_ratio: f32,
    pub(super) resize_window: bool,
    pub(super) resize_texture: bool,
    pub(super) replay_recording: bool,
    pub(super) audio_recording: bool,
    new_genie_code: String,
    shortcut_keybinds: BTreeMap<String, Keybind>,
    joypad_keybinds: [BTreeMap<String, Keybind>; 4],
    pub(super) frame_counter: usize,
    frame_timer: Instant,
    avg_fps: f32,
    messages: Vec<(String, Instant)>,
    pub(super) loaded_rom: Option<String>,
    status: Option<&'static str>,
    pub(super) error: Option<String>,
}

impl Gui {
    /// Create a gui `State`.
    pub fn new(
        window: Arc<Window>,
        event_proxy: EventLoopProxy<NesEvent>,
        texture: SizedTexture,
        cfg: Config,
    ) -> Self {
        // Default auto to current config until a ROM is loaded
        let cart_aspect_ratio = cfg.deck.region.aspect_ratio();
        Self {
            initialized: false,
            window,
            title: Config::WINDOW_TITLE.to_string(),
            event_proxy,
            debounced_events: HashMap::new(),
            texture,
            paused: false,
            menu_height: 0.0,
            new_genie_code: String::new(),
            preferences_open: false,
            preferences_tab: PreferencesTab::Emulation,
            keybinds_tab: KeybindsTab::Shortcuts,
            keybinds_open: false,
            pending_keybind: None,
            about_open: false,
            cpu_debugger_open: false,
            ppu_debugger_open: false,
            apu_debugger_open: false,
            cart_aspect_ratio,
            resize_window: false,
            resize_texture: false,
            replay_recording: false,
            audio_recording: false,
            shortcut_keybinds: Action::shortcuts()
                .into_iter()
                .map(ActionBindings::empty)
                .chain(cfg.input.shortcut_bindings())
                .map(|b| (b.action.to_string(), (b.action, b.bindings)))
                .collect::<BTreeMap<_, _>>(),
            joypad_keybinds: [Player::One, Player::Two, Player::Three, Player::Four].map(
                |player| {
                    Action::joypad()
                        .into_iter()
                        .map(|action| ActionBindings::empty_player(action, player))
                        .chain(cfg.input.joypad_bindings(player))
                        .map(|b| (b.action.to_string(), (b.action, b.bindings)))
                        .collect::<BTreeMap<_, _>>()
                },
            ),
            frame_counter: 0,
            frame_timer: Instant::now(),
            avg_fps: 60.0,
            messages: Vec::new(),
            loaded_rom: None,
            status: None,
            error: None,
        }
    }

    pub fn add_message<S>(&mut self, text: S)
    where
        S: Into<String>,
    {
        let text = text.into();
        info!("{text}");
        self.messages.push((text, Instant::now() + MSG_TIMEOUT));
    }

    pub fn add_debounced_event(&mut self, id: &'static str, event: impl Into<NesEvent>) {
        self.debounced_events
            .entry(id)
            .and_modify(|(_, instant)| *instant = Instant::now())
            .or_insert((event.into(), Instant::now()));
    }

    /// Send a custom event to the event loop.
    pub fn send_event(&mut self, event: impl Into<NesEvent>) {
        let event = event.into();
        trace!("Gui event: {event:?}");
        if let Err(err) = self.event_proxy.send_event(event) {
            error!("failed to send nes event: {err:?}");
            std::process::exit(1);
        }
    }

    pub fn aspect_ratio(&self, cfg: &Config) -> f32 {
        if cfg.deck.region.is_auto() {
            self.cart_aspect_ratio
        } else {
            cfg.deck.region.aspect_ratio()
        }
    }

    /// Create the UI.
    pub fn ui(&mut self, ctx: &Context, cfg: &mut Config) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        if !self.initialized {
            self.initialize(ctx);
        }
        self.handle_debounced_events();
        self.handle_pending_keybind(ctx, cfg);

        TopBottomPanel::top("menu_bar")
            .show_animated(ctx, cfg.renderer.show_menubar, |ui| self.menu_bar(ui, cfg));
        CentralPanel::default()
            .frame(Frame::none())
            .show(ctx, |ui| self.nes_frame(ui, cfg));

        // TODO: show confirm quit dialog?

        let mut preferences_open = self.preferences_open;
        egui::Window::new("Preferences")
            .open(&mut preferences_open)
            .enabled(self.pending_keybind.is_none())
            .show(ctx, |ui| self.preferences(ui, cfg));
        self.preferences_open = preferences_open;

        let mut keybinds_open = self.keybinds_open;
        egui::Window::new("Keybinds")
            .open(&mut keybinds_open)
            .enabled(self.pending_keybind.is_none())
            .show(ctx, |ui| self.keybinds(ui, cfg));
        self.keybinds_open = keybinds_open;

        let mut about_open = self.about_open;
        egui::Window::new("About TetaNES")
            .open(&mut about_open)
            .enabled(self.pending_keybind.is_none())
            .show(ctx, |ui| self.about(ui));
        self.about_open = about_open;

        #[cfg(feature = "profiling")]
        puffin_egui::show_viewport_if_enabled(ctx);
    }

    fn initialize(&mut self, ctx: &Context) {
        let mut fonts = FontDefinitions::default();
        let font_name = String::from("pixeloid-sans");
        let font_data = FontData::from_static(include_bytes!("../../../assets/pixeloid-sans.ttf"));
        fonts.font_data.insert(font_name.clone(), font_data);
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .expect("proportional font family defined")
            .insert(0, font_name.clone());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .expect("monospace font family defined")
            .insert(0, font_name);
        ctx.set_fonts(fonts);

        self.initialized = true;
    }

    fn handle_debounced_events(&mut self) {
        let debounced_events = mem::take(&mut self.debounced_events);
        for (id, (event, instant)) in debounced_events {
            if instant.elapsed() >= Duration::from_millis(300) {
                self.send_event(event);
            } else {
                self.debounced_events.insert(id, (event, instant));
            }
        }
    }

    fn handle_pending_keybind(&mut self, ctx: &Context, cfg: &mut Config) {
        if let Some(ref mut pending_keybind) = self.pending_keybind {
            let mut cancelled = false;
            egui::Window::new("Set Keybind")
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    if let Some((action, player)) = pending_keybind.conflict {
                        if let Some(player) = player {
                            ui.label(format!("Conflict with {action} (Player {player})."));
                        } else {
                            ui.label(format!("Conflict with {action}."));
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Overwrite").clicked() {
                                if let Some(input) = pending_keybind.input {
                                    cfg.input.clear_binding(input);
                                    match pending_keybind.player {
                                        Some(player) => {
                                            if let Some((_, bindings)) = self.joypad_keybinds
                                                [player as usize]
                                                .get_mut(action.as_ref())
                                            {
                                                for i in bindings.iter_mut() {
                                                    if i == &Some(input) {
                                                        *i = None;
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            if let Some((_, bindings)) =
                                                self.shortcut_keybinds.get_mut(action.as_ref())
                                            {
                                                for i in bindings.iter_mut() {
                                                    if i == &Some(input) {
                                                        *i = None;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                pending_keybind.conflict = None;
                            }
                            if ui.button("Cancel").clicked() {
                                cancelled = true;
                                pending_keybind.input = None;
                                pending_keybind.conflict = None;
                            }
                        });
                    } else {
                        ui.label(format!(
                        "Press any key on your keyboard or controller to set a new binding for {}.",
                        pending_keybind.action
                    ));
                    }
                });
            if cancelled {
                return;
            }

            match pending_keybind.input {
                Some(input) => {
                    if pending_keybind.conflict.is_none() {
                        match pending_keybind.player {
                            Some(player) => {
                                self.joypad_keybinds[player as usize]
                                    .entry(pending_keybind.action.to_string())
                                    .and_modify(|(_, bindings)| {
                                        bindings[pending_keybind.binding] = Some(input)
                                    })
                                    .or_insert_with(|| {
                                        let mut bindings = [None, None];
                                        bindings[pending_keybind.binding] = Some(input);
                                        (pending_keybind.action, bindings)
                                    });
                            }
                            None => {
                                self.shortcut_keybinds
                                    .entry(pending_keybind.action.to_string())
                                    .and_modify(|(_, bindings)| {
                                        bindings[pending_keybind.binding] = Some(input)
                                    })
                                    .or_insert_with(|| {
                                        let mut bindings = [None, None];
                                        bindings[pending_keybind.binding] = Some(input);
                                        (pending_keybind.action, bindings)
                                    });
                            }
                        }
                        let binding = cfg.input.bindings.iter_mut().find(|b| {
                            b.action == pending_keybind.action && b.player == pending_keybind.player
                        });
                        match binding {
                            Some(bind) => bind.bindings[pending_keybind.binding] = Some(input),
                            None => cfg.input.bindings.push(ActionBindings {
                                action: pending_keybind.action,
                                player: pending_keybind.player,
                                bindings: [Some(input), None],
                            }),
                        }
                        self.pending_keybind = None;
                        self.send_event(UiEvent::PendingKeybind(false));
                        self.send_event(ConfigEvent::InputBindings);
                    }
                }
                None => {
                    let event = ctx.input(|i| {
                        use egui::Event;
                        for event in &i.events {
                            match *event {
                                Event::Key {
                                    physical_key: Some(key),
                                    pressed,
                                    modifiers,
                                    ..
                                } => {
                                    // TODO: Ignore unsupported key mappings for now as egui supports less
                                    // overall than winit
                                    return Input::try_from((key, modifiers))
                                        .ok()
                                        .map(|input| (input, pressed));
                                }
                                Event::PointerButton {
                                    button, pressed, ..
                                } => {
                                    // TODO: Ignore unsupported key mappings for now as egui supports less
                                    // overall than winit
                                    return Input::try_from(button)
                                        .ok()
                                        .map(|input| (input, pressed));
                                }
                                _ => (),
                            }
                        }
                        None
                    });
                    if let Some((input, pressed)) = event {
                        // Only set on key release
                        if !pressed {
                            pending_keybind.input = Some(input);
                            for bind in &cfg.input.bindings {
                                if bind.bindings.iter().any(|b| b == &Some(input)) {
                                    pending_keybind.conflict = Some((bind.action, bind.player));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui, cfg: &mut Config) {
        // ui.style_mut().spacing.menu_margin = Margin::ZERO;
        let inner_response = menu::bar(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                global_dark_light_mode_switch(ui);
                ui.separator();

                ui.menu_button("📁 File", |ui| self.file_menu(ui, cfg));
                ui.menu_button("🔧 Controls", |ui| self.controls_menu(ui, cfg));
                ui.menu_button("⚙ Config", |ui| self.config_menu(ui, cfg));
                // icon: screen
                ui.menu_button("🖵 Window", |ui| self.window_menu(ui, cfg));
                ui.menu_button("🕷 Debug", |ui| self.debug_menu(ui, cfg));
                ui.toggle_value(&mut self.about_open, "🔎 About");
            });
        });
        let spacing = ui.style().spacing.item_spacing;
        let border = 1.0;
        let height = inner_response.response.rect.height() + spacing.y + border;
        if height != self.menu_height {
            self.menu_height = height;
            self.resize_window = true;
        }
    }

    fn file_menu(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.allocate_space(Vec2::new(MENU_WIDTH, 0.0));
        // NOTE: Due to some platforms file dialogs blocking the event loop,
        // loading requires a round-trip in order for the above pause to
        // get processed.
        if ui
            .add(Button::new("Load ROM...").shortcut_text(self.get_shortcut(UiAction::LoadRom)))
            .clicked()
        {
            self.send_event(EmulationEvent::Pause(true));
            self.send_event(UiEvent::LoadRomDialog);
            ui.close_menu();
        }
        let res = ui.add_enabled_ui(self.loaded_rom.is_some(), |ui| {
            if ui
                .add(
                    Button::new("Load Replay")
                        .shortcut_text(self.get_shortcut(UiAction::LoadReplay)),
                )
                .on_hover_text("Load a replay file for the currently loaded ROM.")
                .clicked()
            {
                self.send_event(EmulationEvent::Pause(true));
                self.send_event(UiEvent::LoadReplayDialog);
                ui.close_menu();
            }
        });
        if self.loaded_rom.is_none() {
            res.response
                .on_hover_text("Replays can only be played when a ROM is loaded.");
        }
        ui.menu_button("Recently Played...", |ui| {
            use tetanes_core::fs;

            if cfg.renderer.recent_roms.is_empty() {
                ui.label("No recent ROMs");
            } else {
                // TODO: add timestamp, save slots, and screenshot
                for rom in &cfg.renderer.recent_roms {
                    if ui.button(fs::filename(rom)).clicked() {
                        self.send_event(EmulationEvent::LoadRomPath(rom.to_path_buf()));
                        ui.close_menu();
                    }
                }
            }
        });

        // TODO: support saves and recent games on wasm? Requires storing the data
        if platform::supports(platform::Feature::Filesystem) {
            ui.separator();

            if ui
                .add(
                    Button::new("Save State")
                        .shortcut_text(self.get_shortcut(DeckAction::SaveState)),
                )
                .on_hover_text("Save the current state to the selected save slot.")
                .clicked()
            {
                self.send_event(EmulationEvent::SaveState(cfg.emulation.save_slot));
            };
            if ui
                .add(
                    Button::new("Load State")
                        .shortcut_text(self.get_shortcut(DeckAction::LoadState)),
                )
                .on_hover_text("Load a previous state from the selected save slot.")
                .clicked()
            {
                self.send_event(EmulationEvent::LoadState(cfg.emulation.save_slot));
            }

            ui.menu_button("Save Slot...", |ui| {
                self.save_slot_radio(ui, cfg, ShowShortcut::Yes)
            });

            ui.separator();

            if ui
                .add(Button::new("Quit").shortcut_text(self.get_shortcut(UiAction::Quit)))
                .clicked()
            {
                self.send_event(UiEvent::Terminate);
                ui.close_menu();
            };
        }
    }

    fn controls_menu(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.allocate_space(Vec2::new(MENU_WIDTH, 0.0));
        let pause_label = if self.paused { "Resume" } else { "Pause" };
        if ui
            .add(Button::new(pause_label).shortcut_text(self.get_shortcut(UiAction::TogglePause)))
            .clicked()
        {
            self.send_event(EmulationEvent::Pause(!self.paused));
            ui.close_menu();
        };
        let mute_label = if cfg.audio.enabled { "Mute" } else { "Unmute" };
        if ui
            .add(Button::new(mute_label).shortcut_text(self.get_shortcut(Setting::ToggleAudio)))
            .clicked()
        {
            cfg.audio.enabled = !cfg.audio.enabled;
            self.send_event(ConfigEvent::AudioEnabled(cfg.audio.enabled));
            ui.close_menu();
        };

        ui.separator();

        let res = ui.add_enabled_ui(cfg.emulation.rewind, |ui| {
            if ui
                .add(
                    Button::new("Instant Rewind")
                        .shortcut_text(self.get_shortcut(Feature::InstantRewind)),
                )
                .on_hover_text("Instantly rewind state to a previous point.")
                .clicked()
            {
                self.send_event(EmulationEvent::InstantRewind);
                ui.close_menu();
            };
        });
        if !cfg.emulation.rewind {
            res.response
                .on_hover_text("Rewinding can be enabled under the `Config` menu.");
        }
        if ui
            .add(
                Button::new("Reset")
                    .shortcut_text(self.get_shortcut(DeckAction::Reset(ResetKind::Soft))),
            )
            .on_hover_text("Emulate a soft reset of the NES.")
            .clicked()
        {
            self.send_event(EmulationEvent::Reset(ResetKind::Soft));
            ui.close_menu();
        };
        if ui
            .add(
                Button::new("Power Cycle")
                    .shortcut_text(self.get_shortcut(DeckAction::Reset(ResetKind::Hard))),
            )
            .on_hover_text("Emulate a power cycle of the NES.")
            .clicked()
        {
            self.send_event(EmulationEvent::Reset(ResetKind::Hard));
            ui.close_menu();
        };

        if platform::supports(platform::Feature::Filesystem) {
            ui.separator();

            if ui
                .add(
                    Button::new("Screenshot")
                        .shortcut_text(self.get_shortcut(Feature::TakeScreenshot)),
                )
                .clicked()
            {
                self.send_event(EmulationEvent::Screenshot);
                ui.close_menu();
            };
            let replay_label = if self.replay_recording {
                "Stop Replay Recording"
            } else {
                "Record Replay"
            };
            if ui
                .add(
                    Button::new(replay_label)
                        .shortcut_text(self.get_shortcut(Feature::ToggleReplayRecording)),
                )
                .on_hover_text("Record or stop recording a game replay file.")
                .clicked()
            {
                self.send_event(EmulationEvent::ReplayRecord(!self.replay_recording));
                ui.close_menu();
            };
            let audio_label = if self.audio_recording {
                "Stop Audio Recording"
            } else {
                "Record Audio"
            };
            if ui
                .add(
                    Button::new(audio_label)
                        .shortcut_text(self.get_shortcut(Feature::ToggleAudioRecording)),
                )
                .on_hover_text("Record or stop recording a audio file.")
                .clicked()
            {
                self.send_event(EmulationEvent::AudioRecord(!self.audio_recording));
                ui.close_menu();
            };
        }
    }

    fn config_menu(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.allocate_space(Vec2::new(MENU_WIDTH, 0.0));
        self.cycle_acurate_checkbox(ui, cfg, ShowShortcut::Yes);
        self.zapper_checkbox(ui, cfg, ShowShortcut::Yes);
        self.rewind_checkbox(ui, cfg, ShowShortcut::Yes);
        self.overscan_checkbox(ui, cfg, ShowShortcut::Yes);

        ui.separator();

        ui.menu_button("Speed...", |ui| {
            let speed = cfg.emulation.speed;
            if ui
                .add(
                    Button::new("Increment")
                        .shortcut_text(self.get_shortcut(Setting::IncrementSpeed)),
                )
                .clicked()
            {
                let new_speed = cfg.increment_speed();
                if speed != new_speed {
                    self.send_event(ConfigEvent::Speed(new_speed));
                }
            }
            if ui
                .add(
                    Button::new("Decrement")
                        .shortcut_text(self.get_shortcut(Setting::DecrementSpeed)),
                )
                .clicked()
            {
                let new_speed = cfg.decrement_speed();
                if speed != new_speed {
                    self.send_event(ConfigEvent::Speed(new_speed));
                }
            }
            self.speed_slider(ui, cfg);
        });
        ui.menu_button("Run Ahead...", |ui| self.run_ahead_slider(ui, cfg));

        ui.separator();

        ui.menu_button("Video Filter...", |ui| self.video_filter_radio(ui, cfg));
        ui.menu_button("Nes Region...", |ui| self.nes_region_radio(ui, cfg));
        ui.menu_button("Four Player...", |ui| self.four_player_radio(ui, cfg));
        ui.menu_button("Game Genie Codes...", |ui| self.genie_codes_entry(ui, cfg));

        ui.separator();

        if ui
            .add(Button::new("Preferences").shortcut_text(self.get_shortcut(Menu::Preferences)))
            .clicked()
        {
            self.preferences_open = !self.preferences_open;
            ui.close_menu();
        }
        if ui
            .add(Button::new("Keybinds").shortcut_text(self.get_shortcut(Menu::Keybinds)))
            .clicked()
        {
            self.keybinds_open = !self.keybinds_open;
            ui.close_menu();
        };
    }

    fn window_menu(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.allocate_space(Vec2::new(MENU_WIDTH, 0.0));
        ui.menu_button("Window Scale...", |ui| {
            let scale = cfg.renderer.scale;
            if ui
                .add(
                    Button::new("Increment")
                        .shortcut_text(self.get_shortcut(Setting::IncrementScale)),
                )
                .clicked()
            {
                let new_scale = cfg.increment_scale();
                if scale != new_scale {
                    self.resize_window = true;
                    self.resize_texture = true;
                }
            }
            if ui
                .add(
                    Button::new("Decrement")
                        .shortcut_text(self.get_shortcut(Setting::DecrementScale)),
                )
                .clicked()
            {
                let new_scale = cfg.decrement_scale();
                if scale != new_scale {
                    self.resize_window = true;
                    self.resize_texture = true;
                }
            }
            self.window_scale_radio(ui, cfg);
        });

        ui.separator();

        if platform::supports(platform::Feature::WindowMinMax) {
            if ui.button("Maximize").clicked() {
                self.window.set_maximized(true);
                ui.close_menu();
            };
            if ui.button("Minimize").clicked() {
                self.window.set_minimized(true);
                ui.close_menu();
            };
        }

        self.fullscreen_checkbox(ui, cfg, ShowShortcut::Yes);

        ui.separator();

        self.menubar_checkbox(ui, cfg, ShowShortcut::Yes);
        self.fps_checkbox(ui, cfg, ShowShortcut::Yes);
        self.messages_checkbox(ui, cfg, ShowShortcut::Yes);
    }

    fn debug_menu(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.allocate_space(Vec2::new(MENU_WIDTH, 0.0));
        #[cfg(feature = "profiling")]
        {
            let mut profile = puffin::are_scopes_on();
            let profiling_label = if profile {
                "Disable Profiling"
            } else {
                "Enable Profiling"
            };
            ui.checkbox(&mut profile, profiling_label)
                .on_hover_text("Toggle the Puffin profiling window");
            puffin::set_scopes_on(profile);
        }
        with_shortcut(
            ui,
            ShowShortcut::Yes,
            self.get_shortcut(Setting::TogglePerfStats),
            |ui| {
                ui.checkbox(&mut cfg.renderer.show_perf_stats, "Enable Perf Stats")
                    .on_hover_text("Enable a performance statistics overlay");
            },
        );
        with_shortcut(
            ui,
            ShowShortcut::Yes,
            self.get_shortcut(Debug::Toggle(Debugger::Cpu)),
            |ui| {
                ui.toggle_value(&mut self.cpu_debugger_open, "CPU Debugger")
                    .on_hover_text("Toggle the CPU Debugger.");
            },
        );
        with_shortcut(
            ui,
            ShowShortcut::Yes,
            self.get_shortcut(Debug::Toggle(Debugger::Ppu)),
            |ui| {
                ui.toggle_value(&mut self.ppu_debugger_open, "PPU Debugger")
                    .on_hover_text("Toggle the PPU Debugger.");
            },
        );
        with_shortcut(
            ui,
            ShowShortcut::Yes,
            self.get_shortcut(Debug::Toggle(Debugger::Apu)),
            |ui| {
                ui.toggle_value(&mut self.apu_debugger_open, "APU Debugger")
                    .on_hover_text("Toggle the APU Debugger.");
            },
        );
        ui.add_enabled_ui(self.paused && self.loaded_rom.is_some(), |ui| {
            let res = with_shortcut(
                ui,
                ShowShortcut::Yes,
                self.get_shortcut(Debug::Step(DebugStep::Into)),
                |ui| {
                    ui.button("Step Into")
                        .on_hover_text("Step a single CPU instruction.")
                },
            );
            if res.clicked() {
                self.send_event(EmulationEvent::DebugStep(DebugStep::Into));
            }
            let res = with_shortcut(
                ui,
                ShowShortcut::Yes,
                self.get_shortcut(Debug::Step(DebugStep::Out)),
                |ui| {
                    ui.button("Step Out")
                        .on_hover_text("Step out of the current CPU function.")
                },
            );
            if res.clicked() {
                self.send_event(EmulationEvent::DebugStep(DebugStep::Out));
            }
            let res = with_shortcut(
                ui,
                ShowShortcut::Yes,
                self.get_shortcut(Debug::Step(DebugStep::Over)),
                |ui| {
                    ui.button("Step Over")
                        .on_hover_text("Step over the next CPU instruction.")
                },
            );
            if res.clicked() {
                self.send_event(EmulationEvent::DebugStep(DebugStep::Over));
            }
            let res = with_shortcut(
                ui,
                ShowShortcut::Yes,
                self.get_shortcut(Debug::Step(DebugStep::Scanline)),
                |ui| {
                    ui.button("Step Scanline")
                        .on_hover_text("Step an entire PPU scanline.")
                },
            );
            if res.clicked() {
                self.send_event(EmulationEvent::DebugStep(DebugStep::Scanline));
            }
            let res = with_shortcut(
                ui,
                ShowShortcut::Yes,
                self.get_shortcut(Debug::Step(DebugStep::Frame)),
                |ui| {
                    ui.button("Step Frame")
                        .on_hover_text("Step an entire PPU Frame.")
                },
            );
            if res.clicked() {
                self.send_event(EmulationEvent::DebugStep(DebugStep::Frame));
            }
        });
    }

    fn nes_frame(&mut self, ui: &mut Ui, cfg: &mut Config) {
        CentralPanel::default()
            .frame(Frame::none())
            .show_inside(ui, |ui| {
                let layout = Layout {
                    main_dir: Direction::TopDown,
                    main_align: Align::Center,
                    cross_align: Align::Center,
                    ..Default::default()
                };
                ui.with_layout(layout, |ui| {
                    let image = Image::from_texture(self.texture)
                        .maintain_aspect_ratio(true)
                        .shrink_to_fit();
                    let frame_resp = ui.add(image).on_hover_cursor(if cfg.deck.zapper {
                        CursorIcon::Crosshair
                    } else {
                        CursorIcon::Default
                    });

                    if cfg.deck.zapper {
                        if let Some(pos) = frame_resp.hover_pos() {
                            let width = Ppu::WIDTH as f32;
                            let height = Ppu::HEIGHT as f32;
                            // Normalize x/y to 0..=1 and scale to PPU dimensions
                            let x =
                                ((pos.x - frame_resp.rect.min.x) / frame_resp.rect.width()) * width;
                            let y = ((pos.y - frame_resp.rect.min.y) / frame_resp.rect.height())
                                * height;
                            if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
                                self.send_event(EmulationEvent::ZapperAim((
                                    x.round() as u32,
                                    y.round() as u32,
                                )));
                            }
                        }
                    }
                });
            });

        if cfg.renderer.show_fps {
            Frame::canvas(ui.style()).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::LEFT).with_main_wrap(true), |ui| {
                    if self.frame_timer.elapsed() >= Duration::from_millis(200) {
                        self.avg_fps =
                            self.frame_counter as f32 / self.frame_timer.elapsed().as_secs_f32();
                        self.frame_counter = 0;
                        self.frame_timer = Instant::now();
                    }
                    ui.label(format!("FPS: {:.2}", self.avg_fps));
                });
            });
        }

        if cfg.renderer.show_messages && (!self.messages.is_empty() || self.error.is_some()) {
            Frame::canvas(ui.style()).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::LEFT).with_main_wrap(true), |ui| {
                    self.message_bar(ui);
                    self.error_bar(ui);
                });
            });
        }

        if self.status.is_some() {
            Frame::canvas(ui.style()).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::LEFT).with_main_wrap(true), |ui| {
                    self.status_bar(ui);
                });
            });
        }
    }

    fn message_bar(&mut self, ui: &mut Ui) {
        let now = Instant::now();
        self.messages.retain(|(_, expires)| now < *expires);
        self.messages.dedup_by(|a, b| a.0.eq(&b.0));
        for (message, _) in self.messages.iter().take(MAX_MESSAGES) {
            ui.label(message);
        }
    }

    fn error_bar(&mut self, ui: &mut Ui) {
        let mut clear_error = false;
        if let Some(ref error) = self.error {
            ui.vertical(|ui| {
                ui.label(RichText::new(error).color(Color32::RED));
                clear_error = ui.button("Clear").clicked();
            });
        }
        if clear_error {
            self.error = None;
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        // TODO: maybe show other statuses like rewinding/playback/recording - bitflags?
        if let Some(status) = self.status {
            ui.label(status);
        }
    }

    fn preferences(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.preferences_tab,
                PreferencesTab::Emulation,
                "Emulation",
            );
            ui.selectable_value(&mut self.preferences_tab, PreferencesTab::Audio, "Audio");
            ui.selectable_value(&mut self.preferences_tab, PreferencesTab::Video, "Video");
            ui.selectable_value(&mut self.preferences_tab, PreferencesTab::Input, "Input");
        });

        ui.separator();

        match self.preferences_tab {
            PreferencesTab::Emulation => self.emulation_preferences(ui, cfg),
            PreferencesTab::Audio => self.audio_preferences(ui, cfg),
            PreferencesTab::Video => self.video_preferences(ui, cfg),
            PreferencesTab::Input => self.input_preferences(ui, cfg),
        }

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Restore Defaults").clicked() {
                cfg.reset();
            }
            if platform::supports(platform::Feature::Filesystem) {
                if let Some(data_dir) = Config::default_data_dir() {
                    if ui.button("Clear Save States").clicked() {
                        match fs::clear_dir(data_dir) {
                            Ok(_) => self.add_message("Save States cleared"),
                            Err(_) => self.add_message("Failed to clear Save States"),
                        }
                    }
                    if ui.button("Clear Recent ROMs").clicked() {
                        cfg.renderer.recent_roms.clear();
                    }
                }
            }
        });
    }

    fn emulation_preferences(&mut self, ui: &mut Ui, cfg: &mut Config) {
        Grid::new("emulation_checkboxes")
            .num_columns(2)
            .spacing([40.0, 4.0])
            .show(ui, |ui| {
                self.cycle_acurate_checkbox(ui, cfg, ShowShortcut::No);
                self.rewind_checkbox(ui, cfg, ShowShortcut::No);
                ui.end_row();

                ui.checkbox(&mut cfg.emulation.auto_load, "Auto-Load");
                ui.checkbox(&mut cfg.emulation.auto_save, "Auto-Save");
            });

        ui.separator();

        Grid::new("emulation_preferences")
            .num_columns(2)
            .spacing([40.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Speed:");
                self.speed_slider(ui, cfg);
                ui.end_row();

                ui.strong("Run Ahead:").on_hover_text(
                    "Simulate a number of frames in the future to reduce input lag.",
                );
                self.run_ahead_slider(ui, cfg);
                ui.end_row();

                ui.strong("Save Slot:")
                    .on_hover_text("Select which slot to use when saving or loading game state.");
                ui.horizontal(|ui| self.save_slot_radio(ui, cfg, ShowShortcut::No));
                ui.end_row();

                ui.strong("Four Player:");
                ui.horizontal(|ui| self.four_player_radio(ui, cfg));
                ui.end_row();

                ui.strong("NES Region:");
                ui.horizontal(|ui| self.nes_region_radio(ui, cfg));
                ui.end_row();

                ui.strong("RAM State:");
                ui.horizontal(|ui| self.ram_state_radio(ui, cfg));
                ui.end_row();
            });
    }

    fn audio_preferences(&mut self, ui: &mut Ui, cfg: &mut Config) {
        if ui
            .checkbox(&mut cfg.audio.enabled, "Enable Audio")
            .clicked()
        {
            self.send_event(ConfigEvent::AudioEnabled(cfg.audio.enabled));
        }

        ui.add_enabled_ui(cfg.audio.enabled, |ui| {
            ui.indent("apu_channels", |ui| {
                let channels = &mut cfg.deck.channels_enabled;
                Grid::new("apu_channels")
                    .spacing([40.0, 4.0])
                    .num_columns(2)
                    .show(ui, |ui| {
                        if ui.checkbox(&mut channels[0], "Enable Pulse1").clicked() {
                            self.send_event(ConfigEvent::ApuChannelEnabled((
                                Channel::Pulse1,
                                channels[0],
                            )));
                        }
                        if ui.checkbox(&mut channels[3], "Enable Noise").clicked() {
                            self.send_event(ConfigEvent::ApuChannelEnabled((
                                Channel::Noise,
                                channels[3],
                            )));
                        }
                        ui.end_row();

                        if ui.checkbox(&mut channels[1], "Enable Pulse2").clicked() {
                            self.send_event(ConfigEvent::ApuChannelEnabled((
                                Channel::Pulse2,
                                channels[1],
                            )));
                        }
                        if ui.checkbox(&mut channels[4], "Enable DMC").clicked() {
                            self.send_event(ConfigEvent::ApuChannelEnabled((
                                Channel::Dmc,
                                channels[4],
                            )));
                        }
                        ui.end_row();

                        if ui.checkbox(&mut channels[2], "Enable Triangle").clicked() {
                            self.send_event(ConfigEvent::ApuChannelEnabled((
                                Channel::Triangle,
                                channels[2],
                            )));
                        }
                        if ui.checkbox(&mut channels[5], "Enable Mapper").clicked() {
                            self.send_event(ConfigEvent::ApuChannelEnabled((
                                Channel::Mapper,
                                channels[5],
                            )));
                        }
                        ui.end_row();
                    });

                ui.separator();

                Grid::new("audio_settings")
                    .spacing([40.0, 4.0])
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.strong("Buffer Size:");
                        if ui
                            .add(
                                DragValue::new(&mut cfg.audio.buffer_size)
                                    .speed(10)
                                    .clamp_range(0..=8192)
                                    .suffix(" samples"),
                            )
                            .changed()
                        {
                            self.add_debounced_event(
                                "audio_buffer",
                                ConfigEvent::AudioBuffer(cfg.audio.buffer_size),
                            );
                        }
                        ui.end_row();

                        ui.strong("Latency:");
                        let mut latency = cfg.audio.latency.as_millis() as u64;
                        let changed = ui
                            .add(
                                DragValue::new(&mut latency)
                                    .speed(1)
                                    .clamp_range(0..=1000)
                                    .suffix(" ms"),
                            )
                            .changed();
                        if changed {
                            cfg.audio.latency = Duration::from_millis(latency);
                            self.add_debounced_event(
                                "audio_latency",
                                ConfigEvent::AudioLatency(cfg.audio.latency),
                            );
                        }
                        ui.end_row();
                    });
            });
        });
    }

    fn video_preferences(&mut self, ui: &mut Ui, cfg: &mut Config) {
        Grid::new("video_checkboxes")
            .spacing([40.0, 4.0])
            .num_columns(3)
            .show(ui, |ui| {
                self.menubar_checkbox(ui, cfg, ShowShortcut::No);
                self.fps_checkbox(ui, cfg, ShowShortcut::No);
                self.messages_checkbox(ui, cfg, ShowShortcut::No);
                ui.end_row();

                self.overscan_checkbox(ui, cfg, ShowShortcut::No);
                self.fullscreen_checkbox(ui, cfg, ShowShortcut::No);
                if ui.checkbox(&mut cfg.renderer.vsync, "VSync").clicked() {
                    self.send_event(ConfigEvent::Vsync(cfg.renderer.vsync));
                }
            });

        Grid::new("video_preferences")
            .num_columns(5)
            .spacing([40.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Window Scale:");
                self.window_scale_radio(ui, cfg);
                ui.end_row();

                ui.strong("Video Filter:");
                self.video_filter_radio(ui, cfg);
            });
    }

    fn input_preferences(&mut self, ui: &mut Ui, cfg: &mut Config) {
        Grid::new("input_preferences")
            .num_columns(2)
            .spacing([40.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                self.zapper_checkbox(ui, cfg, ShowShortcut::No);
                if ui
                    .checkbox(&mut cfg.deck.concurrent_dpad, "Enable Concurrent D-Pad")
                    .clicked()
                {
                    self.send_event(ConfigEvent::ConcurrentDpad(cfg.deck.concurrent_dpad));
                }
            });
    }

    fn keybinds(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.keybinds_tab, KeybindsTab::Shortcuts, "Shortcuts");
            ui.selectable_value(
                &mut self.keybinds_tab,
                KeybindsTab::Joypad(Player::One),
                "Player1",
            );
            ui.selectable_value(
                &mut self.keybinds_tab,
                KeybindsTab::Joypad(Player::Two),
                "Player2",
            );
            ui.selectable_value(
                &mut self.keybinds_tab,
                KeybindsTab::Joypad(Player::Three),
                "Player3",
            );
            ui.selectable_value(
                &mut self.keybinds_tab,
                KeybindsTab::Joypad(Player::Four),
                "Player4",
            );
        });

        ui.separator();

        match self.keybinds_tab {
            KeybindsTab::Shortcuts => {
                ScrollArea::vertical().show(ui, |ui| {
                    Grid::new("shortcut_keybinds")
                        .num_columns(1)
                        .spacing([40.0, 4.0])
                        .striped(true)
                        .show(ui, |ui| self.keybind_list(ui, cfg, None));
                });
            }
            KeybindsTab::Joypad(player) => {
                ScrollArea::vertical().show(ui, |ui| {
                    Grid::new("player_keybinds")
                        .num_columns(1)
                        .spacing([40.0, 4.0])
                        .striped(true)
                        .show(ui, |ui| self.keybind_list(ui, cfg, Some(player)));
                });
            }
        }
    }

    fn keybind_list(&mut self, ui: &mut Ui, cfg: &mut Config, player: Option<Player>) {
        ui.heading("Action");
        ui.heading("Binding #1");
        ui.heading("Binding #2");
        ui.end_row();

        let mut changed = false;
        let keybinds = match player {
            None => &mut self.shortcut_keybinds,
            Some(player) => &mut self.joypad_keybinds[player as usize],
        };
        for (action, input) in keybinds.values_mut() {
            ui.strong(action.to_string());
            for (slot, input) in input.iter_mut().enumerate() {
                let res = ui
                    .button(
                        input
                            .map(format_input)
                            .unwrap_or_else(|| String::from("click to set")),
                    )
                    .on_hover_text("Right-click to clear binding.");
                if res.clicked() {
                    self.pending_keybind = Some(SetKeybind {
                        action: *action,
                        player,
                        binding: slot,
                        input: None,
                        conflict: None,
                    });
                } else if res.secondary_clicked() {
                    if let Some(input) = input.take() {
                        cfg.input.clear_binding(input);
                        changed = true;
                    }
                }
            }
            ui.end_row();
        }

        if changed {
            self.send_event(ConfigEvent::InputBindings);
        } else if self.pending_keybind.is_some() {
            self.send_event(UiEvent::PendingKeybind(true));
        }
    }

    fn about(&mut self, ui: &mut Ui) {
        Grid::new("version")
            .num_columns(2)
            .spacing([40.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Version:");
                ui.label(env!("CARGO_PKG_VERSION").to_string());
                ui.end_row();

                ui.strong("GitHub:");
                ui.hyperlink("https://github.com/lukexor/tetanes");
                ui.end_row();
            });

        if platform::supports(platform::Feature::Filesystem) {
            ui.separator();
            Grid::new("directories")
                .num_columns(2)
                .spacing([40.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    if let Some(config_dir) = Config::default_config_dir() {
                        ui.strong("Preferences:");
                        ui.label(format!("{}", config_dir.display()));
                        ui.end_row();
                    }
                    if let Some(data_dir) = Config::default_data_dir() {
                        ui.strong("Save States, Battery-Backed Ram, Replays: ");
                        ui.label(format!("{}", data_dir.display()));
                        ui.end_row();
                    }
                    if let Some(picture_dir) = Config::default_picture_dir() {
                        ui.strong("Screenshots: ");
                        ui.label(format!("{}", picture_dir.display()));
                        ui.end_row();
                    }
                    if let Some(audio_dir) = Config::default_audio_dir() {
                        ui.strong("Audio Recordings: ");
                        ui.label(format!("{}", audio_dir.display()));
                        ui.end_row();
                    }
                });
        }
    }

    fn save_slot_radio(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        for slot in 1..=8 {
            with_shortcut(
                ui,
                show,
                self.get_shortcut(DeckAction::SetSaveSlot(slot)),
                |ui| {
                    ui.radio_value(&mut cfg.emulation.save_slot, slot, slot.to_string());
                },
            );
        }
    }

    fn speed_slider(&mut self, ui: &mut Ui, cfg: &mut Config) {
        if ui
            .add(
                Slider::new(&mut cfg.emulation.speed, 0.25..=2.0)
                    .step_by(0.25)
                    .suffix("%"),
            )
            .changed()
        {
            self.add_debounced_event("speed", ConfigEvent::Speed(cfg.emulation.speed));
        }
    }

    fn run_ahead_slider(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.add(Slider::new(&mut cfg.emulation.run_ahead, 0..=4));
    }

    fn cycle_acurate_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        let res = with_shortcut(
            ui,
            show,
            self.get_shortcut(Setting::ToggleCycleAccurate),
            |ui| {
                ui.checkbox(&mut cfg.deck.cycle_accurate, "Cycle Accurate")
                    .on_hover_text(
                        "Enables more accurate NES emulation at a slight cost in performance.",
                    )
            },
        );
        if res.clicked() {
            self.send_event(ConfigEvent::CycleAccurate(cfg.deck.cycle_accurate));
        }
        if res.hovered() {}
    }

    fn rewind_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        let res = with_shortcut(
            ui,
            show,
            self.get_shortcut(Setting::ToggleRewinding),
            |ui| {
                ui.checkbox(&mut cfg.emulation.rewind, "Enable Rewinding")
                    .on_hover_text("Enable instant and visual rewinding. Increases memory usage.")
            },
        );
        if res.clicked() {
            self.send_event(ConfigEvent::RewindEnabled(cfg.emulation.rewind));
        }
    }

    fn zapper_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        let res = with_shortcut(
            ui,
            show,
            self.get_shortcut(DeckAction::ToggleZapperConnected),
            |ui| {
                ui.checkbox(&mut cfg.deck.zapper, "Enable Zapper Gun")
                    .on_hover_text("Enable the Zapper Light Gun for games that support it.")
            },
        );
        if res.clicked() {
            self.send_event(ConfigEvent::ZapperConnected(cfg.deck.zapper));
        }
    }

    fn overscan_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        let res = with_shortcut(ui, show, self.get_shortcut(Setting::ToggleOverscan), |ui| {
            ui.checkbox(&mut cfg.renderer.hide_overscan, "Hide Overscan")
                .on_hover_text("Traditional CRT displays would crop the top and bottom edges of the image. Disable this to show the overscan.")
        });
        self.resize_texture = res.clicked();
    }

    fn video_filter_radio(&mut self, ui: &mut Ui, cfg: &mut Config) {
        let filter = cfg.deck.filter;
        ui.radio_value(&mut cfg.deck.filter, VideoFilter::Pixellate, "Pixellate")
            .on_hover_text("Basic pixel-perfect rendering");
        ui.radio_value(&mut cfg.deck.filter, VideoFilter::Ntsc, "Ntsc")
            .on_hover_text(
                "Emulate traditional NTSC rendering where chroma spills over into luma.",
            );
        if filter != cfg.deck.filter {
            self.send_event(ConfigEvent::VideoFilter(cfg.deck.filter));
        }
    }

    fn four_player_radio(&mut self, ui: &mut Ui, cfg: &mut Config) {
        let four_player = cfg.deck.four_player;
        ui.radio_value(&mut cfg.deck.four_player, FourPlayer::Disabled, "Disabled");
        ui.radio_value(
            &mut cfg.deck.four_player,
            FourPlayer::FourScore,
            "Four Score",
        )
        .on_hover_text("Enable NES Four Score for games that support 4 players.");
        ui.radio_value(
            &mut cfg.deck.four_player,
            FourPlayer::Satellite,
            "Satellite",
        )
        .on_hover_text("Enable NES Satellite for games that support 4 players.");
        if four_player != cfg.deck.four_player {
            self.send_event(ConfigEvent::FourPlayer(cfg.deck.four_player));
        }
    }

    fn nes_region_radio(&mut self, ui: &mut Ui, cfg: &mut Config) {
        let region = cfg.deck.region;
        ui.radio_value(&mut cfg.deck.region, NesRegion::Auto, "Auto")
            .on_hover_text("Auto-detect region based on loaded ROM.");
        ui.radio_value(&mut cfg.deck.region, NesRegion::Ntsc, "NTSC")
            .on_hover_text("Emulate NTSC timing and aspect-ratio.");
        ui.radio_value(&mut cfg.deck.region, NesRegion::Pal, "PAL")
            .on_hover_text("Emulate PAL timing and aspect-ratio.");
        ui.radio_value(&mut cfg.deck.region, NesRegion::Dendy, "Dendy")
            .on_hover_text("Emulate Dendy timing and aspect-ratio.");
        if region != cfg.deck.region {
            self.resize_window = true;
            self.resize_texture = true;
            self.send_event(ConfigEvent::Region(cfg.deck.region));
        }
    }

    fn ram_state_radio(&mut self, ui: &mut Ui, cfg: &mut Config) {
        let ram_state = cfg.deck.ram_state;
        ui.radio_value(&mut cfg.deck.ram_state, RamState::AllZeros, "All 0x00")
            .on_hover_text("Clear startup RAM to all zeroes for predictable emulation.");
        ui.radio_value(&mut cfg.deck.ram_state, RamState::AllOnes, "All 0xFF")
            .on_hover_text("Clear startup RAM to all ones for predictable emulation.");
        ui.radio_value(&mut cfg.deck.ram_state, RamState::Random, "Random")
            .on_hover_text("Randomize startup RAM, which some games use as a basic RNG seed.");
        if ram_state != cfg.deck.ram_state {
            self.send_event(ConfigEvent::RamState(cfg.deck.ram_state));
        }
    }

    fn genie_codes_entry(&mut self, ui: &mut Ui, cfg: &mut Config) {
        ui.strong("Add Genie Code:");
        ui.horizontal(|ui| {
            let res = ui.text_edit_singleline(&mut self.new_genie_code);
            if (res.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                || ui.button("➕").clicked()
            {
                match GenieCode::parse(&self.new_genie_code) {
                    Ok(hex) => {
                        let code = GenieCode::from_raw(mem::take(&mut self.new_genie_code), hex);
                        if !cfg.deck.genie_codes.contains(&code) {
                            cfg.deck.genie_codes.push(code.clone());
                            self.send_event(ConfigEvent::GenieCodeAdded(code));
                        }
                    }
                    Err(err) => self.add_message(err.to_string()),
                }
            }
        });

        if !cfg.deck.genie_codes.is_empty() {
            ui.separator();
            ui.strong("Current Genie Codes:");
            cfg.deck.genie_codes.retain(|genie| {
                ui.horizontal(|ui| {
                    ui.label(genie.code());
                    // icon: waste basket
                    if ui.button("🗑").clicked() {
                        self.send_event(ConfigEvent::GenieCodeRemoved(genie.code().to_string()));
                        false
                    } else {
                        true
                    }
                })
                .inner
            });
        }
    }

    fn menubar_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        with_shortcut(ui, show, self.get_shortcut(Setting::ToggleMenubar), |ui| {
            ui.checkbox(&mut cfg.renderer.show_menubar, "Show Menu Bar")
                .on_hover_text("Show the menu bar.");
        });
    }

    fn fps_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        with_shortcut(ui, show, self.get_shortcut(Setting::ToggleFps), |ui| {
            ui.checkbox(&mut cfg.renderer.show_fps, "Show FPS")
                .on_hover_text("Show an average FPS counter in the corner of the window.");
        });
    }

    fn messages_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        with_shortcut(ui, show, self.get_shortcut(Setting::ToggleMessages), |ui| {
            ui.checkbox(&mut cfg.renderer.show_messages, "Show Messages")
                .on_hover_text("Show shortcut and emulator messages.");
        });
    }

    fn window_scale_radio(&mut self, ui: &mut Ui, cfg: &mut Config) {
        let scale = cfg.renderer.scale;
        ui.radio_value(&mut cfg.renderer.scale, 1.0, "1x");
        ui.radio_value(&mut cfg.renderer.scale, 2.0, "2x");
        ui.radio_value(&mut cfg.renderer.scale, 3.0, "3x");
        ui.radio_value(&mut cfg.renderer.scale, 4.0, "4x");
        ui.radio_value(&mut cfg.renderer.scale, 5.0, "5x");
        if scale != cfg.renderer.scale {
            self.resize_window = true;
            self.resize_texture = true;
        }
    }

    fn fullscreen_checkbox(&mut self, ui: &mut Ui, cfg: &mut Config, show: ShowShortcut) {
        let res = with_shortcut(
            ui,
            show,
            self.get_shortcut(Setting::ToggleFullscreen),
            |ui| ui.checkbox(&mut cfg.renderer.fullscreen, "Fullscreen"),
        );
        if res.clicked() {
            self.window.set_fullscreen(
                cfg.renderer
                    .fullscreen
                    .then_some(Fullscreen::Borderless(None)),
            );
        }
    }

    fn get_shortcut(&self, action: impl Into<Action>) -> String {
        let action = action.into();
        self.shortcut_keybinds
            .get(action.as_ref())
            .and_then(|(_, binding)| binding[0])
            .map(format_input)
            .unwrap_or_default()
    }
}

#[derive(Debug, Copy, Clone)]
#[must_use]
pub enum ShowShortcut {
    Yes,
    No,
}

/// Helper method for adding a shortcut label to any widget since only `Button` implements
/// `shortcut_text` currently.
fn with_shortcut<R>(
    ui: &mut Ui,
    show: ShowShortcut,
    shortcut: impl Into<RichText>,
    f: impl FnOnce(&mut Ui) -> R,
) -> R {
    match show {
        ShowShortcut::Yes => {
            ui.horizontal(|ui| {
                let res = f(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.weak(shortcut);
                });
                res
            })
            .inner
        }
        ShowShortcut::No => f(ui),
    }
}

fn format_input(input: Input) -> String {
    match input {
        Input::Key(keycode, modifiers) => {
            let mut s = String::with_capacity(32);
            if modifiers.contains(ModifiersState::CONTROL) {
                s += "Ctrl";
            }
            if modifiers.contains(ModifiersState::SHIFT) {
                if !s.is_empty() {
                    s += "+";
                }
                s += "Shift";
            }
            if modifiers.contains(ModifiersState::ALT) {
                if !s.is_empty() {
                    s += "+";
                }
                s += "Alt";
            }
            if modifiers.contains(ModifiersState::SUPER) {
                if !s.is_empty() {
                    s += "+";
                }
                s += "Super";
            }
            let ch = match keycode {
                KeyCode::Backquote => "`",
                KeyCode::Backslash | KeyCode::IntlBackslash => "\\",
                KeyCode::BracketLeft => "[",
                KeyCode::BracketRight => "]",
                KeyCode::Comma | KeyCode::NumpadComma => ",",
                KeyCode::Digit0 => "0",
                KeyCode::Digit1 => "1",
                KeyCode::Digit2 => "2",
                KeyCode::Digit3 => "3",
                KeyCode::Digit4 => "4",
                KeyCode::Digit5 => "5",
                KeyCode::Digit6 => "6",
                KeyCode::Digit7 => "7",
                KeyCode::Digit8 => "8",
                KeyCode::Digit9 => "9",
                KeyCode::Equal => "=",
                KeyCode::KeyA => "A",
                KeyCode::KeyB => "B",
                KeyCode::KeyC => "C",
                KeyCode::KeyD => "D",
                KeyCode::KeyE => "E",
                KeyCode::KeyF => "F",
                KeyCode::KeyG => "G",
                KeyCode::KeyH => "H",
                KeyCode::KeyI => "I",
                KeyCode::KeyJ => "J",
                KeyCode::KeyK => "K",
                KeyCode::KeyL => "L",
                KeyCode::KeyM => "M",
                KeyCode::KeyN => "N",
                KeyCode::KeyO => "O",
                KeyCode::KeyP => "P",
                KeyCode::KeyQ => "Q",
                KeyCode::KeyR => "R",
                KeyCode::KeyS => "S",
                KeyCode::KeyT => "T",
                KeyCode::KeyU => "U",
                KeyCode::KeyV => "V",
                KeyCode::KeyW => "W",
                KeyCode::KeyX => "X",
                KeyCode::KeyY => "Y",
                KeyCode::KeyZ => "Z",
                KeyCode::Minus | KeyCode::NumpadSubtract => "-",
                KeyCode::Period | KeyCode::NumpadDecimal => ".",
                KeyCode::Quote => "'",
                KeyCode::Semicolon => ";",
                KeyCode::Slash | KeyCode::NumpadDivide => "/",
                KeyCode::Backspace | KeyCode::NumpadBackspace => "Backspace",
                KeyCode::Enter | KeyCode::NumpadEnter => "Enter",
                KeyCode::Space => "Space",
                KeyCode::Tab => "Tab",
                KeyCode::Delete => "Delete",
                KeyCode::End => "End",
                KeyCode::Help => "Help",
                KeyCode::Home => "Home",
                KeyCode::Insert => "Ins",
                KeyCode::PageDown => "PageDown",
                KeyCode::PageUp => "PageUp",
                KeyCode::ArrowDown => "Down",
                KeyCode::ArrowLeft => "Left",
                KeyCode::ArrowRight => "Right",
                KeyCode::ArrowUp => "Up",
                KeyCode::Numpad0 => "Num0",
                KeyCode::Numpad1 => "Num1",
                KeyCode::Numpad2 => "Num2",
                KeyCode::Numpad3 => "Num3",
                KeyCode::Numpad4 => "Num4",
                KeyCode::Numpad5 => "Num5",
                KeyCode::Numpad6 => "Num6",
                KeyCode::Numpad7 => "Num7",
                KeyCode::Numpad8 => "Num8",
                KeyCode::Numpad9 => "Num9",
                KeyCode::NumpadAdd => "+",
                KeyCode::NumpadEqual => "=",
                KeyCode::NumpadHash => "#",
                KeyCode::NumpadMultiply => "*",
                KeyCode::NumpadParenLeft => "(",
                KeyCode::NumpadParenRight => ")",
                KeyCode::NumpadStar => "*",
                KeyCode::Escape => "Escape",
                KeyCode::Fn => "Fn",
                KeyCode::F1 => "F1",
                KeyCode::F2 => "F2",
                KeyCode::F3 => "F3",
                KeyCode::F4 => "F4",
                KeyCode::F5 => "F5",
                KeyCode::F6 => "F6",
                KeyCode::F7 => "F7",
                KeyCode::F8 => "F8",
                KeyCode::F9 => "F9",
                KeyCode::F10 => "F10",
                KeyCode::F11 => "F11",
                KeyCode::F12 => "F12",
                KeyCode::F13 => "F13",
                KeyCode::F14 => "F14",
                KeyCode::F15 => "F15",
                KeyCode::F16 => "F16",
                KeyCode::F17 => "F17",
                KeyCode::F18 => "F18",
                KeyCode::F19 => "F19",
                KeyCode::F20 => "F20",
                KeyCode::F21 => "F21",
                KeyCode::F22 => "F22",
                KeyCode::F23 => "F23",
                KeyCode::F24 => "F24",
                KeyCode::F25 => "F25",
                KeyCode::F26 => "F26",
                KeyCode::F27 => "F27",
                KeyCode::F28 => "F28",
                KeyCode::F29 => "F29",
                KeyCode::F30 => "F30",
                KeyCode::F31 => "F31",
                KeyCode::F32 => "F32",
                KeyCode::F33 => "F33",
                KeyCode::F34 => "F34",
                KeyCode::F35 => "F35",
                _ => "",
            };
            if !ch.is_empty() {
                if !s.is_empty() {
                    s += "+";
                }
                s += ch;
            }
            s.shrink_to_fit();
            s
        }
        Input::Mouse(button) => match button {
            MouseButton::Left => String::from("Left Click"),
            MouseButton::Right => String::from("Right Click"),
            MouseButton::Middle => String::from("Middle Click"),
            MouseButton::Back => String::from("Back Click"),
            MouseButton::Forward => String::from("Forward Click"),
            MouseButton::Other(id) => format!("Button {id} Click"),
        },
    }
}

impl TryFrom<Input> for KeyboardShortcut {
    type Error = ();

    fn try_from(val: Input) -> Result<Self, Self::Error> {
        if let Input::Key(keycode, modifier_state) = val {
            Ok(KeyboardShortcut {
                logical_key: winit_keycode_into_egui(keycode).ok_or(())?,
                modifiers: winit_modifiers_into_egui(modifier_state),
            })
        } else {
            Err(())
        }
    }
}

impl TryFrom<(Key, Modifiers)> for Input {
    type Error = ();
    fn try_from((key, modifiers): (Key, Modifiers)) -> Result<Self, Self::Error> {
        let keycode = egui_key_into_winit(key).ok_or(())?;
        let modifiers = egui_modifiers_into_winit(modifiers);
        Ok(Input::Key(keycode, modifiers))
    }
}

impl TryFrom<PointerButton> for Input {
    type Error = ();
    fn try_from(button: PointerButton) -> Result<Self, Self::Error> {
        Ok(Input::Mouse(egui_pointer_btn_into_winit(button).ok_or(())?))
    }
}

const fn winit_keycode_into_egui(keycode: KeyCode) -> Option<Key> {
    let key = match keycode {
        KeyCode::Backslash => Key::Backslash,
        KeyCode::BracketLeft => Key::OpenBracket,
        KeyCode::BracketRight => Key::CloseBracket,
        KeyCode::Comma => Key::Comma,
        KeyCode::Digit0 => Key::Num0,
        KeyCode::Digit1 => Key::Num1,
        KeyCode::Digit2 => Key::Num2,
        KeyCode::Digit3 => Key::Num3,
        KeyCode::Digit4 => Key::Num4,
        KeyCode::Digit5 => Key::Num5,
        KeyCode::Digit6 => Key::Num6,
        KeyCode::Digit7 => Key::Num7,
        KeyCode::Digit8 => Key::Num8,
        KeyCode::Digit9 => Key::Num9,
        KeyCode::Equal => Key::Equals,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,
        KeyCode::Minus => Key::Minus,
        KeyCode::Period => Key::Period,
        KeyCode::Semicolon => Key::Semicolon,
        KeyCode::Slash => Key::Slash,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter => Key::Enter,
        KeyCode::Space => Key::Space,
        KeyCode::Tab => Key::Tab,
        KeyCode::Delete => Key::Delete,
        KeyCode::End => Key::End,
        KeyCode::Home => Key::Home,
        KeyCode::Insert => Key::Insert,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::Numpad0 => Key::Num0,
        KeyCode::Numpad1 => Key::Num1,
        KeyCode::Numpad2 => Key::Num2,
        KeyCode::Numpad3 => Key::Num3,
        KeyCode::Numpad4 => Key::Num4,
        KeyCode::Numpad5 => Key::Num5,
        KeyCode::Numpad6 => Key::Num6,
        KeyCode::Numpad7 => Key::Num7,
        KeyCode::Numpad8 => Key::Num8,
        KeyCode::Numpad9 => Key::Num9,
        KeyCode::NumpadAdd => Key::Plus,
        KeyCode::NumpadBackspace => Key::Backspace,
        KeyCode::NumpadComma => Key::Comma,
        KeyCode::NumpadDecimal => Key::Period,
        KeyCode::NumpadEnter => Key::Enter,
        KeyCode::NumpadEqual => Key::Equals,
        KeyCode::NumpadSubtract => Key::Minus,
        KeyCode::Escape => Key::Escape,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        KeyCode::F13 => Key::F13,
        KeyCode::F14 => Key::F14,
        KeyCode::F15 => Key::F15,
        KeyCode::F16 => Key::F16,
        KeyCode::F17 => Key::F17,
        KeyCode::F18 => Key::F18,
        KeyCode::F19 => Key::F19,
        KeyCode::F20 => Key::F20,
        KeyCode::F21 => Key::F21,
        KeyCode::F22 => Key::F22,
        KeyCode::F23 => Key::F23,
        KeyCode::F24 => Key::F24,
        KeyCode::F25 => Key::F25,
        KeyCode::F26 => Key::F26,
        KeyCode::F27 => Key::F27,
        KeyCode::F28 => Key::F28,
        KeyCode::F29 => Key::F29,
        KeyCode::F30 => Key::F30,
        KeyCode::F31 => Key::F31,
        KeyCode::F32 => Key::F32,
        KeyCode::F33 => Key::F33,
        KeyCode::F34 => Key::F34,
        KeyCode::F35 => Key::F35,
        _ => return None,
    };
    Some(key)
}

const fn egui_key_into_winit(key: Key) -> Option<KeyCode> {
    let keycode = match key {
        Key::Backslash => KeyCode::Backslash,
        Key::OpenBracket => KeyCode::BracketLeft,
        Key::CloseBracket => KeyCode::BracketRight,
        Key::Comma => KeyCode::Comma,
        Key::Num0 => KeyCode::Digit0,
        Key::Num1 => KeyCode::Digit1,
        Key::Num2 => KeyCode::Digit2,
        Key::Num3 => KeyCode::Digit3,
        Key::Num4 => KeyCode::Digit4,
        Key::Num5 => KeyCode::Digit5,
        Key::Num6 => KeyCode::Digit6,
        Key::Num7 => KeyCode::Digit7,
        Key::Num8 => KeyCode::Digit8,
        Key::Num9 => KeyCode::Digit9,
        Key::Equals => KeyCode::Equal,
        Key::A => KeyCode::KeyA,
        Key::B => KeyCode::KeyB,
        Key::C => KeyCode::KeyC,
        Key::D => KeyCode::KeyD,
        Key::E => KeyCode::KeyE,
        Key::F => KeyCode::KeyF,
        Key::G => KeyCode::KeyG,
        Key::H => KeyCode::KeyH,
        Key::I => KeyCode::KeyI,
        Key::J => KeyCode::KeyJ,
        Key::K => KeyCode::KeyK,
        Key::L => KeyCode::KeyL,
        Key::M => KeyCode::KeyM,
        Key::N => KeyCode::KeyN,
        Key::O => KeyCode::KeyO,
        Key::P => KeyCode::KeyP,
        Key::Q => KeyCode::KeyQ,
        Key::R => KeyCode::KeyR,
        Key::S => KeyCode::KeyS,
        Key::T => KeyCode::KeyT,
        Key::U => KeyCode::KeyU,
        Key::V => KeyCode::KeyV,
        Key::W => KeyCode::KeyW,
        Key::X => KeyCode::KeyX,
        Key::Y => KeyCode::KeyY,
        Key::Z => KeyCode::KeyZ,
        Key::Minus => KeyCode::Minus,
        Key::Period => KeyCode::Period,
        Key::Semicolon => KeyCode::Semicolon,
        Key::Slash => KeyCode::Slash,
        Key::Backspace => KeyCode::Backspace,
        Key::Enter => KeyCode::Enter,
        Key::Space => KeyCode::Space,
        Key::Tab => KeyCode::Tab,
        Key::Delete => KeyCode::Delete,
        Key::End => KeyCode::End,
        Key::Home => KeyCode::Home,
        Key::Insert => KeyCode::Insert,
        Key::PageDown => KeyCode::PageDown,
        Key::PageUp => KeyCode::PageUp,
        Key::ArrowDown => KeyCode::ArrowDown,
        Key::ArrowLeft => KeyCode::ArrowLeft,
        Key::ArrowRight => KeyCode::ArrowRight,
        Key::ArrowUp => KeyCode::ArrowUp,
        Key::Plus => KeyCode::NumpadAdd,
        Key::Escape => KeyCode::Escape,
        Key::F1 => KeyCode::F1,
        Key::F2 => KeyCode::F2,
        Key::F3 => KeyCode::F3,
        Key::F4 => KeyCode::F4,
        Key::F5 => KeyCode::F5,
        Key::F6 => KeyCode::F6,
        Key::F7 => KeyCode::F7,
        Key::F8 => KeyCode::F8,
        Key::F9 => KeyCode::F9,
        Key::F10 => KeyCode::F10,
        Key::F11 => KeyCode::F11,
        Key::F12 => KeyCode::F12,
        Key::F13 => KeyCode::F13,
        Key::F14 => KeyCode::F14,
        Key::F15 => KeyCode::F15,
        Key::F16 => KeyCode::F16,
        Key::F17 => KeyCode::F17,
        Key::F18 => KeyCode::F18,
        Key::F19 => KeyCode::F19,
        Key::F20 => KeyCode::F20,
        Key::F21 => KeyCode::F21,
        Key::F22 => KeyCode::F22,
        Key::F23 => KeyCode::F23,
        Key::F24 => KeyCode::F24,
        Key::F25 => KeyCode::F25,
        Key::F26 => KeyCode::F26,
        Key::F27 => KeyCode::F27,
        Key::F28 => KeyCode::F28,
        Key::F29 => KeyCode::F29,
        Key::F30 => KeyCode::F30,
        Key::F31 => KeyCode::F31,
        Key::F32 => KeyCode::F32,
        Key::F33 => KeyCode::F33,
        Key::F34 => KeyCode::F34,
        Key::F35 => KeyCode::F35,
        _ => return None,
    };
    Some(keycode)
}

fn winit_modifiers_into_egui(modifier_state: ModifiersState) -> Modifiers {
    Modifiers {
        alt: modifier_state.alt_key(),
        ctrl: modifier_state.control_key(),
        shift: modifier_state.shift_key(),
        #[cfg(target_os = "macos")]
        mac_cmd: modifier_state.super_key(),
        #[cfg(not(target_os = "macos"))]
        mac_cmd: false,
        #[cfg(target_os = "macos")]
        command: modifier_state.super_key(),
        #[cfg(not(target_os = "macos"))]
        command: modifier_state.control_key(),
    }
}

fn egui_modifiers_into_winit(modifiers: Modifiers) -> ModifiersState {
    let mut modifiers_state = ModifiersState::empty();
    if modifiers.shift {
        modifiers_state |= ModifiersState::SHIFT;
    }
    if modifiers.ctrl {
        modifiers_state |= ModifiersState::CONTROL;
    }
    if modifiers.alt {
        modifiers_state |= ModifiersState::ALT;
    }
    #[cfg(target_os = "macos")]
    if modifiers.mac_cmd {
        modifiers_state |= ModifiersState::SUPER;
    }
    // TODO: egui doesn't seem to support SUPER on Windows/Linux
    modifiers_state
}

const fn egui_pointer_btn_into_winit(button: PointerButton) -> Option<MouseButton> {
    let button = match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        _ => return None,
    };
    Some(button)
}
