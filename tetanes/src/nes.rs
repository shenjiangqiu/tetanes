//! User Interface representing the the NES Control Deck

use crate::platform::{BuilderExt, EventLoopExt, Initialize};
use config::Config;
use emulation::Emulation;
use event::{NesEvent, State};
use renderer::{BufferPool, Renderer};
use std::sync::Arc;
use winit::{
    event_loop::{EventLoop, EventLoopBuilder, EventLoopProxy},
    window::{Fullscreen, Window, WindowBuilder},
};

pub mod action;
pub mod audio;
pub mod config;
pub mod emulation;
pub mod event;
pub mod input;
pub mod renderer;

/// Represents all the NES Emulation state.
#[derive(Debug)]
pub struct Nes {
    pub(crate) cfg: Config,
    pub(crate) window: Arc<Window>,
    pub(crate) emulation: Emulation,
    pub(crate) renderer: Renderer,
    // Only used by wasm currently
    #[allow(unused)]
    pub(crate) event_proxy: EventLoopProxy<NesEvent>,
    pub(crate) state: State,
}

impl Nes {
    /// Begins emulation by starting the game engine loop.
    ///
    /// # Errors
    ///
    /// If engine fails to build or run, then an error is returned.
    pub async fn run(cfg: Config) -> anyhow::Result<()> {
        // Set up window, events and NES state
        let event_loop = EventLoopBuilder::<NesEvent>::with_user_event().build()?;
        let mut nes = Nes::new(cfg, &event_loop).await?;
        event_loop
            .run_platform(move |event, window_target| nes.event_loop(event, window_target))?;

        Ok(())
    }

    /// Create the NES emulation.
    async fn new(cfg: Config, event_loop: &EventLoop<NesEvent>) -> anyhow::Result<Self> {
        let window = Arc::new(Nes::initialize_window(event_loop, &cfg)?);
        let event_proxy = event_loop.create_proxy();
        let frame_pool = BufferPool::new();
        let state = State::new(&cfg);
        let emulation =
            Emulation::initialize(event_proxy.clone(), frame_pool.clone(), cfg.clone())?;
        let renderer = Renderer::initialize(
            event_proxy.clone(),
            Arc::clone(&window),
            frame_pool,
            cfg.clone(),
        )
        .await?;
        let mut nes = Self {
            cfg,
            window,
            emulation,
            renderer,
            event_proxy,
            state,
        };
        nes.initialize()?;

        Ok(nes)
    }

    /// Initializes the window in a platform agnostic way.
    pub fn initialize_window(
        event_loop: &EventLoop<NesEvent>,
        cfg: &Config,
    ) -> anyhow::Result<Window> {
        let window_size = cfg.window_size();
        let texture_size = cfg.texture_size();
        Ok(WindowBuilder::new()
            .with_active(true)
            .with_inner_size(window_size)
            .with_min_inner_size(texture_size)
            .with_title(Config::WINDOW_TITLE)
            // TODO: Support exclusive fullscreen config
            .with_fullscreen(
                cfg.renderer
                    .fullscreen
                    .then_some(Fullscreen::Borderless(None)),
            )
            .with_resizable(true)
            .with_platform()
            .build(event_loop)?)
    }

    fn next_frame(&mut self) {
        #[cfg(feature = "profiling")]
        puffin::GlobalProfiler::lock().new_frame();
        if let Err(err) = self.emulation.request_clock_frame() {
            self.on_error(err);
        }
    }
}
