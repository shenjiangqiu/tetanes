use crate::nes::{
    action::{Action, Debug, DebugStep, Debugger, Feature, Setting, Ui},
    config::InputConfig,
    renderer::gui::Menu,
};
use egui::ahash::{HashMap, HashMapExt};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use tetanes_core::{
    action::Action as DeckAction,
    apu::Channel,
    common::ResetKind,
    input::{JoypadBtn, Player},
    video::VideoFilter,
};
use winit::{
    event::MouseButton,
    keyboard::{KeyCode, ModifiersState},
};

macro_rules! action_binding {
    ($action:expr => $bindings:expr) => {
        ActionBindings {
            action: $action.into(),
            bindings: $bindings,
        }
    };
    ($action:expr => $modifiers:expr, $key:expr) => {
        action_binding!($action => [Some(Input::Key($key, $modifiers)), None])
    };
    ($action:expr => $modifiers1:expr, $key1:expr; $modifiers2:expr, $key2:expr) => {
        action_binding!(
            $action => [Some(Input::Key($key1, $modifiers1)), Some(Input::Key($key2, $modifiers2))]
        )
    };
}

#[allow(unused_macro_rules)]
macro_rules! shortcut_map {
    (@ $action:expr => $key:expr) => {
        action_binding!($action => ModifiersState::empty(), $key)
    };
    (@ $action:expr => $key1:expr; $key2:expr) => {
        action_binding!($action => ModifiersState::empty(), $key1; ModifiersState::empty(), $key2)
    };
    (@ $action:expr => :$modifiers:expr, $key:expr) => {
        action_binding!($action => $modifiers, $key)
    };
    (@ $action:expr => :$modifiers1:expr, $key1:expr; $key2:expr) => {
        action_binding!($action => $modifiers1, $key1; ModifiersState::empty(), $key2)
    };
    (@ $action:expr => :$modifiers1:expr, $key1:expr; :$modifiers2:expr, $key2:expr) => {
        action_binding!($action => $modifiers1, $key1; $modifiers2, $key2)
    };
    ($({ $action:expr => $(:$modifiers1:expr,) ?$key1:expr$(; $(:$modifiers2:expr,)? $key2:expr)? }),+$(,)?) => {
        vec![$(shortcut_map!(@ $action => $(:$modifiers1,)? $key1$(; $(:$modifiers2,)? $key2)?),)+]
    };
}

macro_rules! mouse_map {
    (@ $action:expr => $button:expr) => {
        action_binding!($action => [Some(Input::Mouse($button)), None])
    };
    ($({ $action:expr => $button:expr }),+$(,)?) => {
        vec![$(mouse_map!(@ $action => $button),)+]
    };
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[must_use]
pub enum Input {
    Key(KeyCode, ModifiersState),
    Mouse(MouseButton),
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[must_use]
pub struct ActionBindings {
    pub action: Action,
    pub bindings: [Option<Input>; 2],
}

impl ActionBindings {
    pub fn empty(action: Action) -> Self {
        Self {
            action,
            bindings: Default::default(),
        }
    }

    pub fn default_shortcuts() -> Vec<Self> {
        use KeyCode::*;
        const SHIFT: ModifiersState = ModifiersState::SHIFT;
        const CONTROL: ModifiersState = ModifiersState::CONTROL;

        let mut bindings = Vec::with_capacity(64);
        bindings.extend(shortcut_map!(
            { Debug::Step(DebugStep::Frame) => :SHIFT, KeyF },
            { Debug::Step(DebugStep::Into) => KeyC },
            { Debug::Step(DebugStep::Out) => :SHIFT, KeyO },
            { Debug::Step(DebugStep::Over) => KeyO },
            { Debug::Step(DebugStep::Scanline) => :SHIFT, KeyL },
            { Debug::Toggle(Debugger::Apu) => :SHIFT, KeyA },
            { Debug::Toggle(Debugger::Cpu) => :SHIFT, KeyD },
            { Debug::Toggle(Debugger::Ppu) => :SHIFT, KeyP },
            { DeckAction::LoadState => :CONTROL, KeyL },
            { DeckAction::Reset(ResetKind::Hard) => :CONTROL, KeyH },
            { DeckAction::Reset(ResetKind::Soft) => :CONTROL, KeyR },
            { DeckAction::SaveState => :CONTROL, KeyS },
            { DeckAction::SetSaveSlot(1) => :CONTROL, Digit1 },
            { DeckAction::SetSaveSlot(2) => :CONTROL, Digit2 },
            { DeckAction::SetSaveSlot(3) => :CONTROL, Digit3 },
            { DeckAction::SetSaveSlot(4) => :CONTROL, Digit4 },
            { DeckAction::SetSaveSlot(5) => :CONTROL, Digit5 },
            { DeckAction::SetSaveSlot(6) => :CONTROL, Digit6 },
            { DeckAction::SetSaveSlot(7) => :CONTROL, Digit7 },
            { DeckAction::SetSaveSlot(8) => :CONTROL, Digit8 },
            { DeckAction::SetVideoFilter(VideoFilter::Ntsc) => :CONTROL, KeyN },
            { DeckAction::ToggleApuChannel(Channel::Dmc) => :SHIFT, Digit5 },
            { DeckAction::ToggleApuChannel(Channel::Mapper) => :SHIFT, Digit6 },
            { DeckAction::ToggleApuChannel(Channel::Noise) => :SHIFT, Digit4 },
            { DeckAction::ToggleApuChannel(Channel::Pulse1) => :SHIFT, Digit1 },
            { DeckAction::ToggleApuChannel(Channel::Pulse2) => :SHIFT, Digit2 },
            { DeckAction::ToggleApuChannel(Channel::Triangle) => :SHIFT, Digit3 },
            { Feature::InstantRewind => KeyR },
            { Feature::TakeScreenshot => F10 },
            { Feature::ToggleAudioRecording => :SHIFT, KeyR },
            { Feature::ToggleReplayRecording => :SHIFT, KeyV },
            { Feature::VisualRewind => KeyR },
            { Menu::About => F1 },
            { Menu::Keybinds => :CONTROL, KeyK },
            { Menu::Preferences => :CONTROL, KeyP; F2 },
            { Setting::DecrementScale => :SHIFT, Minus },
            { Setting::DecrementSpeed => Minus },
            { Setting::FastForward => Space },
            { Setting::IncrementScale => :SHIFT, Equal },
            { Setting::IncrementSpeed => Equal },
            { Setting::ToggleAudio => :CONTROL, KeyM },
            { Setting::ToggleFullscreen => :CONTROL, Enter },
            { Setting::ToggleMenubar => :CONTROL, KeyE },
            { Ui::LoadRom => :CONTROL, KeyO; F3 },
            { Ui::Quit => :CONTROL, KeyQ },
            { Ui::TogglePause => Escape },
        ));
        bindings.extend(mouse_map!(
            { DeckAction::ZapperTrigger => MouseButton::Left },
            { DeckAction::ZapperAimOffscreen => MouseButton::Right }
        ));
        bindings.shrink_to_fit();

        bindings
    }

    pub fn default_player_bindings(player: Player) -> Vec<Self> {
        use KeyCode::*;

        let mut bindings = Vec::with_capacity(10);
        match player {
            Player::One => bindings.extend(shortcut_map!(
                { (Player::One, JoypadBtn::A) => KeyZ },
                { (Player::One, JoypadBtn::B) => KeyX },
                { (Player::One, JoypadBtn::Down) => ArrowDown },
                { (Player::One, JoypadBtn::Left) => ArrowLeft },
                { (Player::One, JoypadBtn::Right) => ArrowRight },
                { (Player::One, JoypadBtn::Select) => KeyW },
                { (Player::One, JoypadBtn::Start) => KeyQ; Enter },
                { (Player::One, JoypadBtn::TurboA) => KeyA },
                { (Player::One, JoypadBtn::TurboB) => KeyS },
                { (Player::One, JoypadBtn::Up) => ArrowUp },
            )),
            Player::Two => bindings.extend(shortcut_map!(
                { (Player::Two, JoypadBtn::A) => KeyN },
                { (Player::Two, JoypadBtn::B) => KeyM },
                { (Player::Two, JoypadBtn::Down) => KeyK },
                { (Player::Two, JoypadBtn::Left) => KeyJ },
                { (Player::Two, JoypadBtn::Right) => KeyL },
                { (Player::Two, JoypadBtn::Select) => Digit9 },
                { (Player::Two, JoypadBtn::Start) => Digit8 },
                { (Player::Two, JoypadBtn::Up) => KeyI },
            )),
            Player::Three => {
                #[cfg(debug_assertions)]
                bindings.extend(shortcut_map!(
                    { (Player::Three, JoypadBtn::A) => KeyV },
                    { (Player::Three, JoypadBtn::B) => KeyB },
                    { (Player::Three, JoypadBtn::Down) => KeyG },
                    { (Player::Three, JoypadBtn::Left) => KeyF },
                    { (Player::Three, JoypadBtn::Right) => KeyH },
                    { (Player::Three, JoypadBtn::Select) => Digit6 },
                    { (Player::Three, JoypadBtn::Start) => Digit5 },
                    { (Player::Three, JoypadBtn::Up) => KeyT },
                ));
            }
            Player::Four => (),
        };
        bindings.shrink_to_fit();

        bindings
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputBindings(HashMap<Input, Action>);

impl InputBindings {
    pub fn from_input_config(config: &InputConfig) -> Self {
        let mut map = HashMap::with_capacity(256);
        for bind in config
            .shortcuts
            .iter()
            .chain(config.joypad_bindings.iter().flatten())
        {
            for input in bind.bindings.into_iter().flatten() {
                map.insert(input, bind.action);
            }
        }
        map.shrink_to_fit();
        Self(map)
    }
}

impl Deref for InputBindings {
    type Target = HashMap<Input, Action>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for InputBindings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
