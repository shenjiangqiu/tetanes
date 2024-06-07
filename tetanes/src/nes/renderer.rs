use crate::{
    nes::{
        config::Config,
        event::{EmulationEvent, NesEvent, RendererEvent, RunState, SendNesEvent, UiEvent},
        input::Gamepads,
        renderer::{
            gui::{Gui, MessageType},
            texture::Texture,
        },
    },
    platform::{self, BuilderExt, Initialize},
    thread,
};
use anyhow::Context;
use egui::{
    ahash::HashMap, DeferredViewportUiCallback, ImmediateViewport, SystemTheme, Vec2,
    ViewportBuilder, ViewportClass, ViewportCommand, ViewportId, ViewportIdMap, ViewportIdPair,
    ViewportIdSet, ViewportInfo, ViewportOutput, WindowLevel,
};
use egui_wgpu::{winit::Painter, RenderState};
use egui_winit::EventResponse;
use parking_lot::Mutex;
use std::{cell::RefCell, collections::hash_map::Entry, rc::Rc, sync::Arc};
use tetanes_core::{
    fs,
    ppu::Ppu,
    time::{Duration, Instant},
    video::Frame,
};
use thingbuf::{
    mpsc::{blocking::Receiver as BufReceiver, errors::TryRecvError},
    Recycle,
};
use tracing::{debug, error, info, warn};
use winit::{
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{EventLoopProxy, EventLoopWindowTarget},
    window::{Theme, Window, WindowId},
};

pub mod gui;
pub mod texture;

pub const OVERSCAN_TRIM: usize = (4 * Ppu::WIDTH * 8) as usize;

#[derive(Debug)]
#[must_use]
pub struct FrameRecycle;

impl Recycle<Frame> for FrameRecycle {
    fn new_element(&self) -> Frame {
        Frame::new()
    }

    fn recycle(&self, _frame: &mut Frame) {}
}

#[must_use]
pub struct State {
    pub(crate) viewports: ViewportIdMap<Viewport>,
    viewport_from_window: HashMap<WindowId, ViewportId>,
    painter: Rc<RefCell<Painter>>,
    focused: Option<ViewportId>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("viewports", &self.viewports)
            .field("viewport_from_window", &self.viewport_from_window)
            .field("focused", &self.focused)
            .finish_non_exhaustive()
    }
}

#[must_use]
pub struct Viewport {
    ids: ViewportIdPair,
    class: ViewportClass,
    builder: ViewportBuilder,
    info: ViewportInfo,
    viewport_ui_cb: Option<Arc<DeferredViewportUiCallback>>,
    screenshot_requested: bool,
    window: Option<Arc<Window>>,
    pub(crate) egui_state: Option<egui_winit::State>,
    occluded: bool,
}

impl std::fmt::Debug for Viewport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Viewport")
            .field("ids", &self.ids)
            .field("builder", &self.builder)
            .field("info", &self.info)
            .field("screenshot_requested", &self.screenshot_requested)
            .field("window", &self.window)
            .field("occluded", &self.occluded)
            .finish_non_exhaustive()
    }
}

#[must_use]
pub struct Renderer {
    pub(crate) state: Rc<RefCell<State>>,
    frame_rx: BufReceiver<Frame, FrameRecycle>,
    tx: EventLoopProxy<NesEvent>,
    // Only used by the immediate viewport renderer callback
    redraw_tx: Arc<Mutex<EventLoopProxy<NesEvent>>>,
    pub(crate) gui: Rc<RefCell<Gui>>,
    pub(crate) ctx: egui::Context,
    render_state: Option<RenderState>,
    texture: Texture,
    first_frame: bool,
    last_save_time: Instant,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("state", &self.state)
            .field("frame_rx", &self.frame_rx)
            .field("tx", &self.tx)
            .field("redraw_tx", &self.redraw_tx)
            .field("gui", &self.gui)
            .field("ctx", &self.ctx)
            .field("texture", &self.texture)
            .field("first_frame", &self.first_frame)
            .finish_non_exhaustive()
    }
}

#[must_use]
pub struct Resources {
    pub(crate) ctx: egui::Context,
    pub(crate) window: Arc<Window>,
    pub(crate) viewport_builder: ViewportBuilder,
    pub(crate) painter: Painter,
}

impl std::fmt::Debug for Resources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resources")
            .field("window", &self.window)
            .field("viewport_builder", &self.viewport_builder)
            .finish_non_exhaustive()
    }
}

impl Renderer {
    /// Initializes the renderer in a platform-agnostic way.
    pub fn new(
        tx: EventLoopProxy<NesEvent>,
        event_loop: &EventLoopWindowTarget<NesEvent>,
        resources: Resources,
        frame_rx: BufReceiver<Frame, FrameRecycle>,
        cfg: &Config,
    ) -> anyhow::Result<Self> {
        let Resources {
            ctx,
            window,
            viewport_builder,
            painter,
        } = resources;

        let redraw_tx = Arc::new(Mutex::new(tx.clone()));
        ctx.set_request_repaint_callback({
            let redraw_tx = redraw_tx.clone();
            move |info| {
                // IMPORTANT: Wasm can't block
                if let Some(tx) = redraw_tx.try_lock() {
                    tx.nes_event(RendererEvent::RequestRedraw {
                        viewport_id: info.viewport_id,
                        when: Instant::now() + info.delay,
                    });
                } else {
                    tracing::warn!("failed to lock redraw_tx");
                }
            }
        });

        // Platforms like wasm don't easily support multiple viewports, and even if it could spawn
        // multiple canvases for each viewport, the async requirements of wgpu would make it
        // impossible to render until wasm-bindgen gets proper non-blocking async/await support.
        if platform::supports(platform::Feature::Viewports) {
            ctx.set_embed_viewports(cfg.renderer.embed_viewports);
        }

        let max_texture_side = painter.max_texture_side();
        let egui_state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            max_texture_side,
        );
        let mut viewport_from_window = HashMap::default();
        viewport_from_window.insert(window.id(), ViewportId::ROOT);

        let mut viewports = ViewportIdMap::default();
        viewports.insert(
            ViewportId::ROOT,
            Viewport {
                ids: ViewportIdPair::ROOT,
                class: ViewportClass::Root,
                builder: viewport_builder.clone(),
                info: ViewportInfo {
                    minimized: window.is_minimized(),
                    maximized: Some(window.is_maximized()),
                    ..Default::default()
                },
                viewport_ui_cb: None,
                screenshot_requested: false,
                window: Some(Arc::clone(&window)),
                egui_state: Some(egui_state),
                occluded: false,
            },
        );

        let render_state = painter.render_state();
        let (Some(max_texture_side), Some(render_state)) = (max_texture_side, render_state) else {
            anyhow::bail!("render state is not initialized yet");
        };

        let texture_size = cfg.texture_size();
        let texture = Texture::new(
            &render_state.device,
            &mut render_state.renderer.write(),
            texture_size.x.min(max_texture_side as f32) as u32,
            texture_size.y.min(max_texture_side as f32) as u32,
            cfg.deck.region.aspect_ratio(),
            Some("nes frame"),
        );

        let gui = Rc::new(RefCell::new(Gui::new(
            tx.clone(),
            texture.sized_texture(),
            cfg,
        )));

        let state = Rc::new(RefCell::new(State {
            viewports,
            painter: Rc::new(RefCell::new(painter)),
            viewport_from_window,
            focused: Some(ViewportId::ROOT),
        }));

        {
            let tx = tx.clone();
            let state = Rc::downgrade(&state);
            let event_loop: *const EventLoopWindowTarget<NesEvent> = event_loop;
            egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
                if let Some(state) = state.upgrade() {
                    // SAFETY: the event loop lives longer than the Rcs we just upgraded above.
                    match unsafe { event_loop.as_ref() } {
                        Some(event_loop) => {
                            Self::render_immediate_viewport(&tx, event_loop, ctx, &state, viewport);
                        }
                        None => tracing::error!(
                            "failed to get event_loop in set_immediate_viewport_renderer"
                        ),
                    }
                } else {
                    warn!("set_immediate_viewport_renderer called after window closed");
                }
            });
        }

        if let Err(err) = Self::load(&ctx) {
            tracing::error!("{err:?}");
        }

        Ok(Self {
            state,
            frame_rx,
            tx,
            redraw_tx,
            ctx,
            gui,
            render_state: Some(render_state),
            texture,
            first_frame: true,
            last_save_time: Instant::now(),
        })
    }

    pub fn destroy(&mut self) {
        let State {
            viewports,
            viewport_from_window,
            painter,
            ..
        } = &mut *self.state.borrow_mut();
        viewports.clear();
        viewport_from_window.clear();
        let mut painter = painter.borrow_mut();
        painter.gc_viewports(&ViewportIdSet::default());
        painter.destroy();
    }

    pub fn root_window_id(&self) -> Option<WindowId> {
        self.window_id_for_viewport(ViewportId::ROOT)
    }

    pub fn window_id_for_viewport(&self, viewport_id: ViewportId) -> Option<WindowId> {
        self.state
            .borrow()
            .viewports
            .get(&viewport_id)
            .and_then(|viewport| viewport.window.as_ref())
            .map(|window| window.id())
    }

    pub fn viewport_id_for_window(&self, window_id: WindowId) -> Option<ViewportId> {
        let state = self.state.borrow();
        state
            .viewport_from_window
            .get(&window_id)
            .and_then(|id| state.viewports.get(id))
            .map(|viewport| viewport.ids.this)
    }

    pub fn root_viewport<R>(&self, reader: impl FnOnce(&Viewport) -> R) -> Option<R> {
        self.state
            .borrow()
            .viewports
            .get(&ViewportId::ROOT)
            .map(reader)
    }

    pub fn root_window(&self) -> Option<Arc<Window>> {
        self.root_viewport(|viewport| viewport.window.clone())
            .flatten()
    }

    pub fn window(&self, window_id: WindowId) -> Option<Arc<Window>> {
        let state = self.state.borrow();
        state
            .viewport_from_window
            .get(&window_id)
            .and_then(|id| state.viewports.get(id))
            .and_then(|viewport| viewport.window.clone())
    }

    pub fn window_size(&self, cfg: &Config) -> Vec2 {
        self.window_size_for_scale(cfg, cfg.renderer.scale)
    }

    pub fn window_size_for_scale(&self, cfg: &Config, scale: f32) -> Vec2 {
        let gui = self.gui.borrow();
        let aspect_ratio = gui.aspect_ratio(cfg);
        let mut window_size = cfg.window_size_for_scale(aspect_ratio, scale);
        window_size.x *= aspect_ratio;
        window_size.y += gui.menu_height;
        window_size
    }

    pub fn all_viewports_occluded(&self) -> bool {
        self.state
            .borrow()
            .viewports
            .values()
            .all(|viewport| viewport.occluded)
    }

    pub fn inner_size(&self) -> Option<PhysicalSize<u32>> {
        self.root_window().map(|win| win.inner_size())
    }

    pub fn fullscreen(&self) -> bool {
        // viewport.info.fullscreen is sometimes stale, so rely on the actual winit state
        self.root_window()
            .map(|win| win.fullscreen().is_some())
            .unwrap_or(false)
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool, embed_viewports: bool) {
        if platform::supports(platform::Feature::Viewports) {
            self.ctx.set_embed_viewports(fullscreen || embed_viewports);
        }
        self.ctx
            .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
        self.ctx
            .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Fullscreen(fullscreen));
    }

    pub fn set_embed_viewports(&mut self, embed: bool) {
        self.ctx.set_embed_viewports(embed);
    }

    pub fn set_always_on_top(&mut self, always_on_top: bool) {
        self.ctx.send_viewport_cmd_to(
            ViewportId::ROOT,
            ViewportCommand::WindowLevel(if always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            }),
        );
    }

    /// Handle event.
    pub fn on_event(&mut self, event: &NesEvent, cfg: &Config) {
        self.gui.borrow_mut().on_event(event);

        if let NesEvent::Renderer(event) = event {
            match event {
                RendererEvent::ViewportResized((viewport_width, _)) => {
                    // This expands the window width to the desired window width if the new viewport
                    // size allows
                    if let Some(window_size) = self.inner_size() {
                        let window_width = window_size.width as f32;
                        let desired_window_size = self.window_size(cfg);
                        let max_width = 0.8 * viewport_width;

                        if window_width < desired_window_size.x && window_width < max_width {
                            // We have room to resize up to desired_window_size
                            self.ctx.send_viewport_cmd_to(
                                ViewportId::ROOT,
                                ViewportCommand::InnerSize(desired_window_size),
                            );
                        }
                    }
                }
                RendererEvent::ToggleFullscreen => {
                    if platform::supports(platform::Feature::Viewports) {
                        self.ctx.set_embed_viewports(
                            cfg.renderer.fullscreen || cfg.renderer.embed_viewports,
                        );
                    }
                    self.ctx
                        .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                    self.ctx.send_viewport_cmd_to(
                        ViewportId::ROOT,
                        ViewportCommand::Fullscreen(cfg.renderer.fullscreen),
                    );
                }
                RendererEvent::RomLoaded(_) => {
                    if self.state.borrow_mut().focused != Some(ViewportId::ROOT) {
                        self.ctx
                            .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                    }
                }
                _ => (),
            }
        }
    }

    fn initialize_all_windows(&mut self, event_loop: &EventLoopWindowTarget<NesEvent>) {
        if self.ctx.embed_viewports() {
            return;
        }

        let State {
            viewports,
            painter,
            viewport_from_window,
            ..
        } = &mut *self.state.borrow_mut();

        for viewport in viewports.values_mut() {
            viewport.initialize_window(
                self.tx.clone(),
                event_loop,
                &self.ctx,
                viewport_from_window,
                painter,
            );
        }
    }

    pub fn rom_loaded(&self) -> bool {
        self.gui.borrow().loaded_rom.is_some()
    }

    /// Handle window event.
    pub fn on_window_event(
        &mut self,
        window_id: WindowId,
        event: &WindowEvent,
        cfg: &Config,
    ) -> EventResponse {
        let viewport_id = self.viewport_id_for_window(window_id);
        match event {
            WindowEvent::Focused(focused) => {
                self.state.borrow_mut().focused = if *focused { viewport_id } else { None };
            }
            // Note: Does not trigger on all platforms
            WindowEvent::Occluded(occluded) => {
                let mut state = self.state.borrow_mut();
                if let Some(viewport) = viewport_id
                    .as_ref()
                    .and_then(|id| state.viewports.get_mut(id))
                {
                    viewport.occluded = *occluded;
                }
            }
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                if let Some(viewport_id) = viewport_id {
                    let mut state = self.state.borrow_mut();
                    if viewport_id == ViewportId::ROOT {
                        self.tx.nes_event(UiEvent::Terminate);
                    } else if let Some(viewport) = state.viewports.get_mut(&viewport_id) {
                        viewport.info.events.push(egui::ViewportEvent::Close);

                        // We may need to repaint both us and our parent to close the window,
                        // and perhaps twice (once to notice the close-event, once again to enforce it).
                        // `request_repaint_of` does a double-repaint though:
                        self.ctx.request_repaint_of(viewport_id);
                        self.ctx.request_repaint_of(viewport.ids.parent);
                    }
                }
            }
            // To support clipboard in wasm, we need to intercept the Paste event so that
            // egui_winit doesn't try to use it's clipboard fallback logic for paste. Associated
            // behavior in the wasm platform layer handles setting the egui_state clipboard text.
            #[cfg(target_arch = "wasm32")]
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        physical_key: winit::keyboard::PhysicalKey::Code(key),
                        ..
                    },
                ..
            } => {
                if let Some(key) = gui::key_from_keycode(*key) {
                    fn is_paste_command(modifiers: egui::Modifiers, keycode: egui::Key) -> bool {
                        keycode == egui::Key::Paste
                            || (modifiers.command && keycode == egui::Key::V)
                            || (cfg!(target_os = "windows")
                                && modifiers.shift
                                && keycode == egui::Key::Insert)
                    }

                    let modifiers = self.ctx.input(|i| i.modifiers);

                    if is_paste_command(modifiers, key) {
                        return EventResponse {
                            consumed: true,
                            repaint: true,
                        };
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(viewport_id) = viewport_id {
                    use std::num::NonZeroU32;
                    if let (Some(width), Some(height)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    {
                        {
                            self.state
                                .borrow_mut()
                                .painter
                                .borrow_mut()
                                .on_window_resized(viewport_id, width, height);
                        }

                        // On some platforms, e.g. wasm, window width is constrained by the
                        // viewport width
                        // This will resize the window if the desired scale is larger than the
                        // actual scale
                        if let Some(window_size) = self.inner_size() {
                            if !self.fullscreen() {
                                let window_width = window_size.width as f32;
                                let desired_window_size = self.window_size(cfg);

                                if window_width < desired_window_size.x {
                                    let aspect_ratio = self.gui.borrow().aspect_ratio(cfg);
                                    let desired_scale = cfg.renderer.scale;
                                    let actual_scale =
                                        window_width / (aspect_ratio * Ppu::WIDTH as f32);

                                    if desired_scale > actual_scale {
                                        let mut window_size =
                                            self.window_size_for_scale(cfg, actual_scale);
                                        window_size.x = window_width;

                                        self.ctx.send_viewport_cmd_to(
                                            ViewportId::ROOT,
                                            ViewportCommand::InnerSize(window_size),
                                        );
                                        self.add_message(
                                            MessageType::Warn,
                                            "Configured window scale exceeds browser width.",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                self.ctx
                    .send_viewport_cmd(ViewportCommand::SetTheme(if *theme == Theme::Light {
                        SystemTheme::Light
                    } else {
                        SystemTheme::Dark
                    }));
            }
            _ => (),
        }

        let mut state = self.state.borrow_mut();
        let mut res = viewport_id
            .and_then(|viewport_id| {
                state.viewports.get_mut(&viewport_id).and_then(|viewport| {
                    Some(
                        viewport
                            .egui_state
                            .as_mut()?
                            .on_window_event(viewport.window.as_deref()?, event),
                    )
                })
            })
            .unwrap_or_default();

        let gui_res = self.gui.borrow_mut().on_window_event(event);
        res.consumed |= gui_res.consumed;
        res.repaint |= gui_res.repaint;

        res
    }

    /// Handle gamepad event updates.
    pub fn on_gamepad_update(&self, gamepads: &Gamepads) -> EventResponse {
        if self.gui.borrow().pending_keybind.is_some() && gamepads.has_events() {
            return EventResponse {
                consumed: true,
                repaint: true,
            };
        }
        EventResponse::default()
    }

    pub fn add_message<S>(&mut self, ty: MessageType, text: S)
    where
        S: Into<String>,
    {
        self.gui.borrow_mut().add_message(ty, text);
    }

    pub fn on_error(&mut self, err: anyhow::Error) {
        error!("error: {err:?}");
        self.tx
            .nes_event(EmulationEvent::RunState(RunState::Paused));
        self.gui.borrow_mut().error = Some(err.to_string());
    }

    pub fn load(ctx: &egui::Context) -> anyhow::Result<()> {
        let path = Config::default_config_dir().join("gui.dat");
        if fs::exists(&path) {
            let data = fs::load_raw(path).context("failed to load gui memory")?;
            let memory = bincode::deserialize(&data).context("failed to deserialize gui memory")?;
            ctx.memory_mut(|mem| {
                *mem = memory;
            });
            info!("Loaded UI state");
        }
        Ok(())
    }

    pub fn auto_save(&mut self, cfg: &Config) -> anyhow::Result<()> {
        let time_since_last_save = Instant::now() - self.last_save_time;
        if time_since_last_save > Duration::from_secs(30) {
            self.save(cfg)?;
        }
        Ok(())
    }

    pub fn save(&mut self, cfg: &Config) -> anyhow::Result<()> {
        cfg.save()?;

        let path = Config::default_config_dir().join("gui.dat");
        self.ctx.memory(|mem| {
            let data = bincode::serialize(&mem).context("failed to serialize gui memory")?;
            fs::save_raw(path, &data).context("failed to save gui memory")
        })?;
        self.last_save_time = Instant::now();

        info!("Saved UI state");

        Ok(())
    }

    pub fn create_window(
        event_loop: &EventLoopWindowTarget<NesEvent>,
        ctx: &egui::Context,
        cfg: &Config,
    ) -> anyhow::Result<(Window, ViewportBuilder)> {
        let window_size = cfg.window_size(cfg.deck.region.aspect_ratio());
        let mut viewport_builder = ViewportBuilder::default()
            .with_app_id(Config::WINDOW_TITLE)
            .with_title(Config::WINDOW_TITLE)
            .with_active(true)
            .with_visible(false) // hide until first frame is rendered on platforms that support it
            .with_inner_size(window_size)
            .with_min_inner_size(Vec2::new(Ppu::WIDTH as f32, Ppu::HEIGHT as f32))
            .with_fullscreen(cfg.renderer.fullscreen)
            .with_resizable(true);
        if cfg.renderer.always_on_top {
            viewport_builder = viewport_builder.with_always_on_top();
        }

        let window_builder =
            egui_winit::create_winit_window_builder(ctx, event_loop, viewport_builder.clone());

        let window = window_builder
            .with_platform(Config::WINDOW_TITLE)
            .build(event_loop)?;

        egui_winit::apply_viewport_builder_to_window(ctx, &window, &viewport_builder);

        debug!("created new window: {:?}", window.id());

        Ok((window, viewport_builder))
    }

    /// Waits for the window to be initialized with a non-zero size. Required during
    /// `create_painter` to correctly create the wgpu surface.
    pub async fn wait_for_window(window: &Arc<Window>) {
        loop {
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                break;
            }
            thread::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn create_painter(window: Arc<Window>) -> anyhow::Result<Painter> {
        use wgpu::Backends;
        // TODO: Support webgpu when more widely supported
        let supported_backends = Backends::VULKAN | Backends::METAL | Backends::DX12 | Backends::GL;
        let mut painter = Painter::new(
            egui_wgpu::WgpuConfiguration {
                supported_backends,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: Some(2),
                ..Default::default()
            },
            1,
            None,
            false,
        );

        // The window must be ready with a non-zero size before `Painter::set_window` is called,
        // otherwise the wgpu surface won't be configured correctly.
        Self::wait_for_window(&window).await;
        painter.set_window(ViewportId::ROOT, Some(window)).await?;

        let adapter_info = painter.render_state().map(|state| state.adapter.get_info());
        if let Some(info) = adapter_info {
            debug!(
                "created new painter for adapter: `{}`. backend: `{}`",
                if info.name.is_empty() {
                    "unknown"
                } else {
                    &info.name
                },
                info.backend.to_str()
            );
        } else {
            debug!("created new painter. Adapter unknown.");
        }

        Ok(painter)
    }

    pub fn recreate_window(&mut self, event_loop: &EventLoopWindowTarget<NesEvent>) {
        if self.ctx.embed_viewports() {
            return;
        }

        let State {
            viewports,
            viewport_from_window,
            painter,
            ..
        } = &mut *self.state.borrow_mut();

        let viewport_builder = viewports
            .get(&ViewportId::ROOT)
            .map(|viewport| viewport.builder.clone())
            .unwrap_or_default();
        let viewport = Self::create_or_update_viewport(
            &self.ctx,
            viewports,
            ViewportIdPair::ROOT,
            ViewportClass::Root,
            viewport_builder,
            None,
            None,
        );

        viewport.initialize_window(
            self.tx.clone(),
            event_loop,
            &self.ctx,
            viewport_from_window,
            painter,
        );
    }

    pub fn drop_window(&mut self) -> Result<(), egui_wgpu::WgpuError> {
        if self.ctx.embed_viewports() {
            return Ok(());
        }
        let mut state = self.state.borrow_mut();
        state.viewports.remove(&ViewportId::ROOT);
        Renderer::set_painter_window(
            self.tx.clone(),
            Rc::clone(&state.painter),
            ViewportId::ROOT,
            None,
        );
        Ok(())
    }

    fn set_painter_window(
        tx: EventLoopProxy<NesEvent>,
        painter: Rc<RefCell<Painter>>,
        viewport_id: ViewportId,
        window: Option<Arc<Window>>,
    ) {
        // This is fine because we won't be yielding. Native platforms call `block_on` and
        // wasm is single-threaded with `spawn_local` and runs on the next microtick.
        #[allow(clippy::await_holding_refcell_ref)]
        thread::spawn(async move {
            if let Err(err) = painter.borrow_mut().set_window(viewport_id, window).await {
                error!("failed to set painter window on viewport id {viewport_id:?}: {err:?}");
                if let Err(err) = tx.send_event(NesEvent::Ui(UiEvent::Terminate)) {
                    error!("failed to send terminate event: {err:?}");
                    std::process::exit(1);
                }
            }
        });
    }

    fn create_or_update_viewport<'a>(
        ctx: &egui::Context,
        viewports: &'a mut ViewportIdMap<Viewport>,
        ids: ViewportIdPair,
        class: ViewportClass,
        mut builder: ViewportBuilder,
        viewport_ui_cb: Option<Arc<DeferredViewportUiCallback>>,
        focused: Option<ViewportId>,
    ) -> &'a mut Viewport {
        if builder.icon.is_none() {
            builder.icon = viewports
                .get_mut(&ids.parent)
                .and_then(|viewport| viewport.builder.icon.clone());
        }

        match viewports.entry(ids.this) {
            Entry::Vacant(entry) => entry.insert(Viewport {
                ids,
                class,
                builder,
                info: Default::default(),
                viewport_ui_cb,
                screenshot_requested: false,
                window: None,
                egui_state: None,
                occluded: false,
            }),
            Entry::Occupied(mut entry) => {
                let viewport = entry.get_mut();
                viewport.class = class;
                viewport.ids.parent = ids.parent;
                viewport.viewport_ui_cb = viewport_ui_cb;

                let (delta_commands, recreate) = viewport.builder.patch(builder);
                if recreate {
                    viewport.window = None;
                    viewport.egui_state = None;
                } else if let Some(window) = &viewport.window {
                    let is_viewport_focused = focused == Some(ids.this);
                    egui_winit::process_viewport_commands(
                        ctx,
                        &mut viewport.info,
                        delta_commands,
                        window,
                        is_viewport_focused,
                        &mut viewport.screenshot_requested,
                    );
                }

                entry.into_mut()
            }
        }
    }

    fn handle_viewport_output(
        ctx: &egui::Context,
        viewports: &mut ViewportIdMap<Viewport>,
        outputs: ViewportIdMap<ViewportOutput>,
        focused: Option<ViewportId>,
    ) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        for (id, output) in outputs {
            let ids = ViewportIdPair::from_self_and_parent(id, output.parent);
            let viewport = Self::create_or_update_viewport(
                ctx,
                viewports,
                ids,
                output.class,
                output.builder,
                output.viewport_ui_cb,
                focused,
            );
            if let Some(window) = viewport.window.as_ref() {
                let is_viewport_focused = focused == Some(id);
                egui_winit::process_viewport_commands(
                    ctx,
                    &mut viewport.info,
                    output.commands,
                    window,
                    is_viewport_focused,
                    &mut viewport.screenshot_requested,
                );
            }
        }
    }

    fn render_immediate_viewport(
        tx: &EventLoopProxy<NesEvent>,
        event_loop: &EventLoopWindowTarget<NesEvent>,
        ctx: &egui::Context,
        state: &RefCell<State>,
        immediate_viewport: ImmediateViewport<'_>,
    ) {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();

        let ImmediateViewport {
            ids,
            builder,
            viewport_ui_cb,
        } = immediate_viewport;

        let input = {
            let State {
                viewports,
                painter,
                viewport_from_window,
                ..
            } = &mut *state.borrow_mut();

            let viewport = Self::create_or_update_viewport(
                ctx,
                viewports,
                ids,
                ViewportClass::Immediate,
                builder,
                None,
                None,
            );

            if viewport.window.is_none() {
                viewport.initialize_window(
                    tx.clone(),
                    event_loop,
                    ctx,
                    viewport_from_window,
                    painter,
                );
            }

            match (&viewport.window, &mut viewport.egui_state) {
                (Some(window), Some(egui_state)) => {
                    egui_winit::update_viewport_info(&mut viewport.info, ctx, window);

                    let mut input = egui_state.take_egui_input(window);
                    input.viewports = viewports
                        .iter()
                        .map(|(id, viewport)| (*id, viewport.info.clone()))
                        .collect();
                    input
                }
                _ => return,
            }
        };

        let output = ctx.run(input, |ctx| {
            viewport_ui_cb(ctx);
        });

        let viewport_id = ids.this;
        let State {
            viewports,
            painter,
            focused,
            ..
        } = &mut *state.borrow_mut();

        if let Some(viewport) = viewports.get_mut(&viewport_id) {
            viewport.info.events.clear();

            if let (Some(window), Some(egui_state)) = (&viewport.window, &mut viewport.egui_state) {
                Renderer::set_painter_window(
                    tx.clone(),
                    Rc::clone(painter),
                    viewport_id,
                    Some(Arc::clone(window)),
                );

                let clipped_primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                painter.borrow_mut().paint_and_update_textures(
                    viewport_id,
                    output.pixels_per_point,
                    [0.0; 4],
                    &clipped_primitives,
                    &output.textures_delta,
                    false,
                );

                egui_state.handle_platform_output(window, output.platform_output);
                Self::handle_viewport_output(ctx, viewports, output.viewport_output, *focused);
            };
        };
    }

    pub fn process_input(
        ctx: &egui::Context,
        state: &Rc<RefCell<State>>,
        gui: &Rc<RefCell<Gui>>,
    ) -> EventResponse {
        let raw_input = {
            let State { viewports, .. } = &mut *state.borrow_mut();

            let Some(viewport) = viewports.get_mut(&egui::ViewportId::ROOT) else {
                return EventResponse::default();
            };
            let Some(window) = &viewport.window else {
                return EventResponse::default();
            };
            if !window.has_focus() {
                return EventResponse::default();
            }
            let Some(egui_state) = viewport.egui_state.as_mut() else {
                return EventResponse::default();
            };
            egui_state.take_egui_input(window)
        };

        let mut output = ctx.run(raw_input, |ctx| {
            gui.borrow_mut()
                .ui(ctx, &mut Gamepads::default(), &mut Config::default());
        });

        let State { viewports, .. } = &mut *state.borrow_mut();

        if let Some(viewport) = viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.info.events.clear();

            let copied_text = std::mem::take(&mut output.platform_output.copied_text);
            if !copied_text.is_empty() {
                #[cfg(target_arch = "wasm32")]
                platform::set_clipboard_text(&copied_text);
            }

            return EventResponse {
                consumed: true,
                repaint: true,
            };
        };

        EventResponse::default()
    }

    /// Request redraw.
    pub fn redraw(
        &mut self,
        window_id: WindowId,
        event_loop: &EventLoopWindowTarget<NesEvent>,
        gamepads: &mut Gamepads,
        cfg: &mut Config,
    ) -> anyhow::Result<()> {
        #[cfg(feature = "profiling")]
        puffin::profile_function!();
        #[cfg(feature = "profiling")]
        puffin::GlobalProfiler::lock().new_frame();

        if self.first_frame {
            self.initialize()?;
        }
        self.initialize_all_windows(event_loop);

        if self.all_viewports_occluded() {
            return Ok(());
        }

        let Some(viewport_id) = self.viewport_id_for_window(window_id) else {
            return Ok(());
        };

        self.handle_resize(viewport_id, cfg);

        let (viewport_ui_cb, raw_input) = {
            let State { viewports, .. } = &mut *self.state.borrow_mut();

            let Some(viewport) = viewports.get_mut(&viewport_id) else {
                return Ok(());
            };
            let Some(window) = &viewport.window else {
                return Ok(());
            };

            if viewport.occluded
                || (viewport_id != ViewportId::ROOT && viewport.viewport_ui_cb.is_none())
            {
                // This will only happen if this is an immediate viewport.
                // That means that the viewport cannot be rendered by itself and needs his parent to be rendered.
                return Ok(());
            }

            egui_winit::update_viewport_info(&mut viewport.info, &self.ctx, window);

            let viewport_ui_cb = viewport.viewport_ui_cb.clone();
            let egui_state = viewport
                .egui_state
                .as_mut()
                .context("failed to get egui_state")?;
            let mut raw_input = egui_state.take_egui_input(window);

            raw_input.viewports = viewports
                .iter()
                .map(|(id, viewport)| (*id, viewport.info.clone()))
                .collect();

            (viewport_ui_cb, raw_input)
        };

        // Copy NES frame buffer before drawing UI because a UI interaction might cause a texture
        // resize tied to a configuration change.
        if viewport_id == ViewportId::ROOT {
            if let Some(render_state) = &self.render_state {
                // We only care about the latest frame
                let mut frame_buffer = self.frame_rx.try_recv_ref();
                while !self.frame_rx.is_empty() {
                    debug!("dropped frame");
                    frame_buffer = self.frame_rx.try_recv_ref();
                }
                match frame_buffer {
                    Ok(frame_buffer) => {
                        self.texture.update(
                            &render_state.queue,
                            if cfg.renderer.hide_overscan
                                && self
                                    .gui
                                    .borrow()
                                    .loaded_region()
                                    .unwrap_or(cfg.deck.region)
                                    .is_ntsc()
                            {
                                &frame_buffer[OVERSCAN_TRIM..frame_buffer.len() - OVERSCAN_TRIM]
                            } else {
                                &frame_buffer
                            },
                        );
                    }
                    Err(TryRecvError::Closed) => {
                        error!("frame channel closed unexpectedly, exiting");
                        event_loop.exit();
                        return Ok(());
                    }
                    _ => (),
                }
            }
        }

        let always_on_top = cfg.renderer.always_on_top;
        let output = self.ctx.run(raw_input, |ctx| {
            if let Some(viewport_ui_cb) = viewport_ui_cb {
                viewport_ui_cb(ctx);
            }
            self.gui.borrow_mut().ui(ctx, gamepads, cfg);
        });

        {
            // Required to get mutable reference again to avoid double borrow when calling gui.ui
            // above because internally gui.ui calls show_viewport_immediate, which requires
            // borrowing state to render
            let State {
                viewports,
                painter,
                focused,
                viewport_from_window,
                ..
            } = &mut *self.state.borrow_mut();

            let Some(viewport) = viewports.get_mut(&viewport_id) else {
                return Ok(());
            };

            viewport.info.events.clear(); // they should have been processed

            let Viewport {
                window: Some(window),
                egui_state: Some(egui_state),
                screenshot_requested,
                ..
            } = viewport
            else {
                return Ok(());
            };

            window.pre_present_notify();

            let clipped_primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
            let screenshot_requested = std::mem::take(screenshot_requested);
            painter.borrow_mut().paint_and_update_textures(
                viewport_id,
                output.pixels_per_point,
                [0.0; 4],
                &clipped_primitives,
                &output.textures_delta,
                screenshot_requested,
            );

            if std::mem::take(&mut self.first_frame) {
                window.set_visible(true);
            }

            let active_viewports_ids: ViewportIdSet =
                output.viewport_output.keys().copied().collect();

            egui_state.handle_platform_output(window, output.platform_output);
            Self::handle_viewport_output(&self.ctx, viewports, output.viewport_output, *focused);

            // Prune dead viewports
            viewports.retain(|id, _| active_viewports_ids.contains(id));
            viewport_from_window.retain(|_, id| active_viewports_ids.contains(id));
            painter.borrow_mut().gc_viewports(&active_viewports_ids);

            // Update viewports
            for (viewport_id, viewport) in viewports {
                if self.gui.borrow().viewport_info_open {
                    egui::Window::new(format!("Viewport Info ({viewport_id:?})"))
                        .open(&mut self.gui.borrow_mut().viewport_info_open)
                        .show(&self.ctx, |ui| viewport.info.ui(ui));
                }
                if always_on_top != cfg.renderer.always_on_top {
                    self.ctx.send_viewport_cmd_to(
                        *viewport_id,
                        ViewportCommand::WindowLevel(if cfg.renderer.always_on_top {
                            WindowLevel::AlwaysOnTop
                        } else {
                            WindowLevel::Normal
                        }),
                    );
                }
            }
        }

        if let Err(err) = self.auto_save(cfg) {
            error!("failed to auto save UI state: {err:?}");
        }

        Ok(())
    }

    fn handle_resize(&mut self, viewport_id: ViewportId, cfg: &Config) {
        if viewport_id == ViewportId::ROOT {
            if self.gui.borrow().resize_window {
                if !self.fullscreen() {
                    self.ctx.send_viewport_cmd_to(
                        ViewportId::ROOT,
                        ViewportCommand::InnerSize(self.window_size(cfg)),
                    );
                }
                self.gui.borrow_mut().resize_window = false;
            }

            if self.gui.borrow().resize_texture {
                let State { painter, .. } = &mut *self.state.borrow_mut();

                if let (Some(max_texture_side), Some(render_state)) =
                    (painter.borrow().max_texture_side(), &self.render_state)
                {
                    let texture_size = cfg.texture_size();
                    self.texture.resize(
                        &render_state.device,
                        &mut render_state.renderer.write(),
                        texture_size.x.min(max_texture_side as f32) as u32,
                        texture_size.y.min(max_texture_side as f32) as u32,
                        self.gui.borrow().aspect_ratio(cfg),
                    );
                    self.gui.borrow_mut().texture = self.texture.sized_texture();
                }
                self.gui.borrow_mut().resize_texture = false;
            }
        }
    }
}

impl Viewport {
    pub fn initialize_window(
        &mut self,
        tx: EventLoopProxy<NesEvent>,
        event_loop: &EventLoopWindowTarget<NesEvent>,
        ctx: &egui::Context,
        viewport_from_window: &mut HashMap<WindowId, ViewportId>,
        painter: &Rc<RefCell<Painter>>,
    ) {
        if self.window.is_some() {
            return;
        }

        let viewport_id = self.ids.this;
        let window_builder =
            egui_winit::create_winit_window_builder(ctx, event_loop, self.builder.clone())
                .with_platform(self.builder.title.as_deref().unwrap_or_default());

        match window_builder.build(event_loop) {
            Ok(window) => {
                egui_winit::apply_viewport_builder_to_window(ctx, &window, &self.builder);

                viewport_from_window.insert(window.id(), viewport_id);
                let window = Arc::new(window);

                Renderer::set_painter_window(
                    tx,
                    Rc::clone(painter),
                    viewport_id,
                    Some(Arc::clone(&window)),
                );

                debug!("created new viewport window: {:?}", window.id());

                self.egui_state = Some(egui_winit::State::new(
                    ctx.clone(),
                    viewport_id,
                    event_loop,
                    Some(window.scale_factor() as f32),
                    painter.borrow().max_texture_side(),
                ));

                self.info.minimized = window.is_minimized();
                self.info.maximized = Some(window.is_maximized());
                self.window = Some(window);
            }
            Err(err) => error!("Failed to create window: {err}"),
        }
    }
}
