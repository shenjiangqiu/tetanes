use crate::sys::platform;
use std::path::{Path, PathBuf};
use winit::{event::Event, event_loop::EventLoopWindowTarget};

pub use platform::*;

/// Trait for any type requiring platform-specific initialization.
pub trait Initialize {
    /// Initialize type.
    fn initialize(&mut self) -> anyhow::Result<()>;
}

/// Extension trait for any builder that provides platform-specific behavior.
pub trait BuilderExt {
    /// Sets platform-specific options.
    fn with_platform(self, title: &str) -> Self;
}

/// Extension trait for `EventLoop` that provides platform-specific behavior.
pub trait EventLoopExt<T> {
    /// Runs the event loop for the current platform.
    fn run_platform<F>(self, event_handler: F) -> anyhow::Result<()>
    where
        F: FnMut(Event<T>, &EventLoopWindowTarget<T>) + 'static;
}

/// Method for platforms supporting opening a file dialog.
pub fn open_file_dialog(
    title: impl Into<String>,
    name: impl Into<String>,
    extensions: &[impl ToString],
    dir: Option<impl AsRef<Path>>,
) -> anyhow::Result<Option<PathBuf>> {
    platform::open_file_dialog_impl(title, name, extensions, dir)
}

/// Platform-specific feature capabilities.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[must_use]
pub enum Feature {
    Filesystem,
    Storage,
    Viewports,
    DeferredViewport,
    Suspend,
    Blocking,
    ConsumePaste,
    AbortOnExit,
}

/// Checks if the current platform supports a given feature.
#[macro_export]
macro_rules! feature {
    ($feature: tt) => {{
        use $crate::platform::Feature::*;
        match $feature {
            Suspend => cfg!(target_os = "android"),
            Filesystem | Storage | Viewports | Blocking | DeferredViewport => {
                cfg!(not(target_arch = "wasm32"))
            }
            // Wasm should never be able to exit
            AbortOnExit | ConsumePaste => cfg!(target_arch = "wasm32"),
        }
    }};
}
