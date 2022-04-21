use crate::{
    common::{config_dir, config_path, NesFormat},
    memory::RamState,
    nes::{
        event::{Action, Input, InputBindings, InputMapping},
        Nes,
    },
    ppu::VideoFilter,
};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter},
    path::PathBuf,
};

pub(crate) const CONFIG: &str = "config.json";
const DEFAULT_CONFIG: &[u8] = include_bytes!("../../config/config.json");
const DEFAULT_SPEED: f32 = 1.0; // 100% - 60 Hz
const MIN_SPEED: f32 = 0.1; // 10% - 6 Hz
const MAX_SPEED: f32 = 4.0; // 400% - 240 Hz

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
/// NES emulation configuration settings.
pub(crate) struct Config {
    pub(crate) rom_path: PathBuf,
    pub(crate) pause_in_bg: bool,
    pub(crate) sound: bool,
    pub(crate) fullscreen: bool,
    pub(crate) vsync: bool,
    pub(crate) filter: VideoFilter,
    pub(crate) concurrent_dpad: bool,
    pub(crate) nes_format: NesFormat,
    pub(crate) ram_state: RamState,
    pub(crate) save_slot: u8,
    pub(crate) scale: f32,
    pub(crate) speed: f32,
    pub(crate) rewind: bool,
    pub(crate) rewind_frames: u32,
    pub(crate) rewind_buffer_size: usize,
    pub(crate) genie_codes: Vec<String>,
    pub(crate) bindings: InputBindings,
    #[serde(skip)]
    pub(crate) input_map: InputMapping,
    // TODO: Runtime log level
}

impl Config {
    pub(crate) fn load() -> Self {
        let config_dir = config_dir();
        if !config_dir.exists() {
            if let Err(err) =
                fs::create_dir_all(config_dir).context("failed to create config directory")
            {
                log::error!("{:?}", err);
            }
        }
        let config_path = config_path(CONFIG);
        if !config_path.exists() {
            if let Err(err) =
                fs::write(&config_path, DEFAULT_CONFIG).context("failed to create default config")
            {
                log::error!("{:?}", err);
            }
        }

        let mut config = File::open(&config_path)
            .with_context(|| format!("failed to open {:?}", config_path))
            .and_then(|file| Ok(serde_json::from_reader::<_, Config>(BufReader::new(file))?))
            .or_else(|err| {
                log::error!(
                    "Invalid config: {:?}, reverting to defaults. Error: {:?}",
                    config_path,
                    err
                );
                serde_json::from_reader(DEFAULT_CONFIG)
            })
            .with_context(|| format!("failed to parse {:?}", config_path))
            .expect("valid configuration");

        for bind in &config.bindings.keys {
            config.input_map.insert(
                Input::Key((bind.player, bind.key, bind.keymod)),
                bind.action,
            );
        }
        for bind in &config.bindings.mouse {
            config
                .input_map
                .insert(Input::Mouse((bind.player, bind.button)), bind.action);
        }
        for bind in &config.bindings.buttons {
            config
                .input_map
                .insert(Input::Button((bind.player, bind.button)), bind.action);
        }
        for bind in &config.bindings.axes {
            config.input_map.insert(
                Input::Axis((bind.player, bind.axis, bind.direction)),
                bind.action,
            );
        }

        config
    }

    pub(crate) fn add_binding(&mut self, input: Input, action: Action) {
        self.input_map.insert(input, action);
        self.bindings.update_from_map(&self.input_map);
    }

    pub(crate) fn remove_binding(&mut self, input: Input) {
        self.input_map.remove(&input);
        self.bindings.update_from_map(&self.input_map);
    }
}

impl Nes {
    pub(crate) fn save_config(&mut self) {
        let path = config_path(CONFIG);
        match File::create(&path)
            .with_context(|| format!("failed to open {:?}", path))
            .map(|file| serde_json::to_writer_pretty(BufWriter::new(file), &self.config))
        {
            Ok(_) => log::info!("Saved configuration"),
            Err(err) => {
                log::error!("{:?}", err);
                self.add_message("Failed to save configuration");
            }
        }
    }

    pub(crate) fn change_speed(&mut self, delta: f32) {
        if self.config.speed % 0.25 != 0.0 {
            // Round to nearest quarter
            self.config.speed = (self.config.speed * 4.0).floor() / 4.0;
        }
        self.config.speed += DEFAULT_SPEED * delta;
        if self.config.speed < MIN_SPEED {
            self.config.speed = MIN_SPEED;
        } else if self.config.speed > MAX_SPEED {
            self.config.speed = MAX_SPEED;
        }
        self.control_deck.set_speed(self.config.speed);
    }

    pub(crate) fn set_speed(&mut self, speed: f32) {
        self.config.speed = speed;
        self.control_deck.set_speed(self.config.speed);
    }
}
