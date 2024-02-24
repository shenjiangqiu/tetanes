use crate::{
    common::ResetKind,
    input::Player,
    nes::{
        config::Config,
        event::{DeckEvent, Event, NesEvent},
        platform::WindowExt,
    },
    platform::time::{Duration, Instant},
    profile,
};
use egui::{
    global_dark_light_mode_switch, load::SizedTexture, menu, Align, Align2, Area, CentralPanel,
    Color32, Context, CursorIcon, Frame, Image, Layout, Margin, Order, RichText, Style,
    TopBottomPanel, Ui, Vec2, Window,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use winit::{
    event_loop::{EventLoop, EventLoopProxy},
    window::Window as WinitWindow,
};

pub const MSG_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_MESSAGES: usize = 3;

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Menu {
    Config(ConfigTab),
    Keybind(Player),
    #[default]
    LoadRom,
    About,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConfigTab {
    General,
    Emulation,
    Audio,
    Video,
}

impl AsRef<str> for ConfigTab {
    fn as_ref(&self) -> &str {
        match self {
            Self::General => "General",
            Self::Emulation => "Emulation",
            Self::Audio => "Audio",
            Self::Video => "Video",
        }
    }
}

#[derive(Debug)]
#[must_use]
pub struct Gui {
    pub window: Arc<WinitWindow>,
    pub event_proxy: EventLoopProxy<Event>,
    pub texture: SizedTexture,
    pub show_menu: bool,
    pub menu_height: f32,
    pub config_open: bool,
    pub keybind_open: bool,
    pub load_rom_open: bool,
    pub about_open: bool,
    pub version: String,
    pub last_frame_duration: Duration,
    pub messages: Vec<(String, Instant)>,
    pub status: Option<&'static str>,
    pub error: Option<String>,
}

impl Gui {
    /// Create a gui `State`.
    pub fn new(
        window: Arc<WinitWindow>,
        event_loop: &EventLoop<Event>,
        texture: SizedTexture,
    ) -> Self {
        Self {
            window,
            event_proxy: event_loop.create_proxy(),
            texture,
            show_menu: true,
            menu_height: 0.0,
            config_open: false,
            keybind_open: false,
            load_rom_open: false,
            about_open: false,
            version: format!("Version: {}", env!("CARGO_PKG_VERSION")),
            last_frame_duration: Duration::default(),
            messages: vec![],
            status: None,
            error: None,
        }
    }

    /// Send a custom event to the event loop.
    pub fn send_event(&mut self, event: impl Into<Event>) {
        let event = event.into();
        log::trace!("Gui event: {event:?}");
        if let Err(err) = self.event_proxy.send_event(event) {
            log::error!("failed to send nes event: {err:?}");
            std::process::exit(1);
        }
    }

    /// Create the UI.
    pub fn ui(&mut self, ctx: &Context, paused: bool, config: &mut Config) {
        profile!();

        TopBottomPanel::top("menu_bar")
            .resizable(true)
            .show_animated(ctx, self.show_menu, |ui| self.menu_bar(ui, paused, config));
        CentralPanel::default()
            .frame(Frame::none())
            .show(ctx, |ui| self.nes_frame(ui, config));

        // TODO: show confirm quit dialog?

        let mut config_open = self.config_open;
        Window::new("Configuration")
            .open(&mut config_open)
            .show(ctx, |ui| self.configuration(ui));
        self.config_open = config_open;

        let mut about_open = self.about_open;
        Window::new("About TetaNES")
            .open(&mut about_open)
            .show(ctx, |ui| self.about(ui, config));
        self.about_open = about_open;

        #[cfg(feature = "profiling")]
        puffin_egui::show_viewport_if_enabled(ctx);
    }

    fn menu_bar(&mut self, ui: &mut Ui, paused: bool, config: &mut Config) {
        ui.style_mut().spacing.menu_margin = Margin::ZERO;
        let inner_response = menu::bar(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                global_dark_light_mode_switch(ui);
                ui.separator();

                ui.menu_button("File", |ui| self.file_menu(ui));
                ui.menu_button("Controls", |ui| self.controls_menu(ui, paused, config));
                ui.menu_button("Emulation", |ui| self.emulation_menu(ui, config));
                ui.menu_button("View", |ui| self.view_menu(ui, config));
                ui.menu_button("Window", |ui| self.window_menu(ui));
                ui.menu_button("Debug", |ui| self.debug_menu(ui));
                ui.toggle_value(&mut self.about_open, "About");
            });
        });
        let height = inner_response.response.rect.height();
        if height != self.menu_height {
            self.menu_height = height;
            self.resize_window(ui.style(), config);
        }
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        if ui.button("Load ROM...").clicked() || self.load_rom_open {
            self.open_load_dialog();
        }
        if ui.button("Recently Played...").clicked() {
            self.todo(ui);
        }
        if ui.button("Load Replay...").clicked() {
            self.todo(ui);
            // self.send_event(DeckEvent::LoadReplay(path));
        }
        // Load Replay
        if ui.button("Configuration").clicked() {
            self.config_open = true;
            ui.close_menu();
        }
        if ui.button("Keybinds").clicked() {
            self.keybind_open = true;
            // Keyboard
            // Controllers
            // combobox
            //   Player 1
            //   Player 2
            //   Player 3
            //   Player 4
            ui.close_menu();
        };
        if ui.button("Reset").clicked() {
            self.send_event(DeckEvent::Reset(ResetKind::Soft));
        };
        if ui.button("Power Cycle").clicked() {
            self.send_event(DeckEvent::Reset(ResetKind::Hard));
        };
        if ui.button("Quit").clicked() {
            self.send_event(NesEvent::Terminate);
        };
    }

    fn controls_menu(&mut self, ui: &mut Ui, paused: bool, config: &mut Config) {
        if ui
            .button(if paused { "Unpause" } else { "Pause" })
            .clicked()
        {
            self.send_event(NesEvent::TogglePause);
        };
        if ui
            .button(if config.audio_enabled {
                "Mute"
            } else {
                "Unmute"
            })
            .clicked()
        {
            config.audio_enabled = !config.audio_enabled;
            self.send_event(DeckEvent::SetAudioEnabled(config.audio_enabled));
        };
        if ui.button("Save State").clicked() {
            self.send_event(DeckEvent::StateSave);
        };
        if ui.button("Load State").clicked() {
            self.send_event(DeckEvent::StateLoad);
        };
        if ui.button("Save Slot...").clicked() {
            self.todo(ui);
        };
        if ui.button("Rewind").clicked() {
            self.todo(ui);
        };
        if ui.button("Toggle Replay Recording").clicked() {
            self.send_event(DeckEvent::ToggleReplayRecord);
        };
        if ui.button("Toggle Audio Recording").clicked() {
            self.send_event(DeckEvent::ToggleAudioRecord);
        };
    }

    fn emulation_menu(&mut self, ui: &mut Ui, config: &mut Config) {
        if ui.button("Speed...").clicked() {
            self.todo(ui);
            // Increase/Decrease/Default
        };
        if ui
            .checkbox(&mut config.deck.zapper, "Enable Zapper Gun")
            .clicked()
        {
            self.send_event(DeckEvent::ZapperConnect(config.deck.zapper));
        }
        // Four Player Mode
        if ui.button("Nes Region...").clicked() {
            // config.set_region(region);
            self.send_event(DeckEvent::SetRegion(config.deck.region));
        }
        // RAM State
        // Concurrent D-Pad
    }

    fn view_menu(&mut self, ui: &mut Ui, config: &mut Config) {
        if ui.button("Scale...").clicked() {
            self.todo(ui);
        };
        ui.checkbox(&mut config.show_fps, "Show FPS");
        ui.checkbox(&mut config.show_messages, "Show Messages");
        if ui
            .checkbox(&mut config.hide_overscan, "Hide Overscan")
            .clicked()
        {
            self.send_event(DeckEvent::SetHideOverscan(config.hide_overscan));
            self.resize_window(ui.style(), config);
        }
        if ui.button("Video Filter...").clicked() {
            self.todo(ui);
        };
        if ui.button("Take Screenshot").clicked() {
            self.send_event(DeckEvent::Screenshot)
        };
    }

    fn window_menu(&mut self, ui: &mut Ui) {
        if ui.button("Maximize").clicked() {
            self.todo(ui);
        };
        if ui.button("Minimize").clicked() {
            self.todo(ui);
        };
        if ui.button("Toggle Fullscreen").clicked() {
            self.todo(ui);
        };
        if ui.button("Hide Menu Bar").clicked() {
            self.todo(ui);
        };
    }

    fn debug_menu(&mut self, ui: &mut Ui) {
        #[cfg(feature = "profiling")]
        {
            let mut profile = puffin::are_scopes_on();
            ui.checkbox(&mut profile, "Toggle profiling");
            crate::profiling::enable(profile);
        }
        if ui.button("Toggle CPU Debugger").clicked() {
            self.todo(ui);
        };
        if ui.button("Toggle PPU Debugger").clicked() {
            self.todo(ui);
        };
        if ui.button("Toggle APU Debugger").clicked() {
            self.todo(ui);
        };
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
        // TODO: Render framerate if enabled
        // TODO: maybe show other statuses like rewinding/playback/recording - bitflags?
        if let Some(status) = self.status {
            ui.label(status);
        }
    }

    fn nes_frame(&mut self, ui: &mut Ui, config: &Config) {
        if !self.messages.is_empty() || self.error.is_some() {
            Area::new("messages")
                .anchor(Align2::LEFT_TOP, Vec2::ZERO)
                .order(Order::Foreground)
                .constrain(true)
                .show(ui.ctx(), |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| {
                        ui.with_layout(Layout::top_down(Align::LEFT).with_main_wrap(true), |ui| {
                            ui.set_width(ui.available_width());
                            self.message_bar(ui);
                            self.error_bar(ui);
                        });
                    });
                });
        }
        if self.status.is_some() {
            Area::new("status")
                .anchor(Align2::LEFT_BOTTOM, Vec2::ZERO)
                .order(Order::Foreground)
                .constrain(true)
                .show(ui.ctx(), |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| {
                        ui.with_layout(Layout::top_down(Align::LEFT).with_main_wrap(true), |ui| {
                            ui.set_width(ui.available_width());
                            self.status_bar(ui);
                        });
                    });
                });
        }
        if config.show_fps {
            ui.label(format!(
                "Last Frame: {:.4}s",
                self.last_frame_duration.as_secs_f32()
            ));
        }
        CentralPanel::default()
            .frame(Frame::none())
            .show_inside(ui, |ui| {
                let frame_resp = ui
                    .add_sized(
                        ui.available_size(),
                        Image::from_texture(self.texture)
                            .maintain_aspect_ratio(true)
                            .shrink_to_fit(),
                    )
                    .on_hover_cursor(if config.deck.zapper {
                        CursorIcon::Crosshair
                    } else {
                        CursorIcon::Default
                    });
                if config.deck.zapper {
                    if let Some(pos) = frame_resp.hover_pos() {
                        let scale = f32::from(config.scale);
                        let x = (pos.x / scale) * config.aspect_ratio;
                        let y = pos.y / scale;
                        if x > 0.0 && y > 0.0 {
                            self.send_event(DeckEvent::ZapperAim((
                                x.round() as u32,
                                y.round() as u32,
                            )));
                        }
                    }
                }
            });
    }

    fn configuration(&mut self, ui: &mut Ui) {
        // button Restore Defaults
        // button Clear Save State
        //
        // General
        //   textedit Config Path
        //   textedit Save Path
        //   textedit Battey-Backed Save Path
        //   checkbox Enable Rewind
        //     textedit Rewind Frames
        //     self.send_event(DeckEvent::SetRewind(enabled));
        //     textedit Rewind Buffer Size (MB)
        //
        // Emulation
        //   combobox Speed
        //   checkbox Enable Zapper Gun
        //   combobox Four Player Mode
        //   combobox NES Region
        //   combobox RAM State
        //   checkbox Concurrent D-Pad
        //     self.send_event(DeckEvent::SetConcurrentDpad(enabled));
        //
        // View
        //   combobox Scale
        //   checkbox Show FPS
        //   checkbox Show Messages
        //   combobox Video Filter
        //   checkbox Show Overscan
        //   combobox Fullscreen Mode
        //   checkbox Enable VSync
        //   checkbox Always On Top
        //
        // Audio
        //   checkbox Enabled
        //   combobox Output Device
        //     self.send_event(DeckEvent::SetAudioDevice(sample_rate));
        //   combobox Sample Rate
        //     self.send_event(DeckEvent::SetAudioSampleRate(sample_rate));
        //   combobox Latency ms
        //     self.send_event(DeckEvent::SetAudioLatency(sample_rate));
        //   checkbox APU Channels
        // let mut save_slot = config.save_slot as usize - 1;
        // if ui.combo_box("Save Slot", &mut save_slot, &["1", "2", "3", "4"], 4)? {
        //     self.config.save_slot = save_slot as u8 + 1;
        // }
    }

    fn about(&mut self, ui: &mut Ui, config: &Config) {
        ui.label(RichText::new(&self.version).strong());
        ui.hyperlink("https://github.com/lukexor/tetanes");

        ui.separator();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Configuration: ").strong());
            ui.label(config.deck.dir.to_string_lossy());
        });

        ui.horizontal(|ui| {
            ui.label(RichText::new("Save States: ").strong());
            ui.label(config.deck.save_dir().to_string_lossy());
        });

        ui.horizontal(|ui| {
            ui.label(RichText::new("Battery-Backed Save States: ").strong());
            ui.label(config.deck.sram_dir().to_string_lossy());
        });
    }

    fn todo(&mut self, ui: &mut Ui) {
        log::warn!("not implemented yet");
    }

    fn open_load_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("nes", &["nes"])
            .pick_file()
        {
            log::info!("loading rom: {path:?}");
            self.load_rom_open = false;
            // Send LoadROM path event
            // self.open_puffin_path(path);
        }
    }

    fn resize_window(&mut self, style: &Style, config: &Config) {
        let spacing = style.spacing.item_spacing;
        let border = 1.0;
        let (inner_size, min_inner_size) =
            config.inner_dimensions_with_spacing(0.0, self.menu_height + spacing.y + border);
        let _ = self.window.request_inner_size(inner_size);
        self.window.set_min_inner_size(Some(min_inner_size));
    }
}
