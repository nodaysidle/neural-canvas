//! NeuralCanvas: winit event loop + wgpu surface hosting the nc-render canvas. Frames are drawn only
//! when something changed (idle = 0% CPU, docs/PERFORMANCE-BUDGET.md). Every edit is a command on the
//! open sketch's session and goes to the store thread; this thread never touches disk.

mod app;
mod chrome;
mod clipboard;
mod gpu;
mod workers;

use app::App;
use nc_core::Session;
use nc_store::{Store, StoreError, StoreEvent};
use std::path::{Path, PathBuf};
use winit::event_loop::{EventLoop, EventLoopProxy};

/// An open store, the sketch it opened on, and whether that sketch is read-only.
pub type Opened = (Store, Session, bool);

/// Wakes the event loop from other threads.
pub enum UserEvent {
    Store(StoreEvent),
    /// The desktop's reduced-motion preference, at startup and whenever it changes.
    ReducedMotion(bool),
    /// SIGTERM / SIGINT / SIGHUP: save and quit.
    Quit,
    /// Elements read from the clipboard, to be centred on the point (world units).
    Pasted(Vec<nc_core::Element>, glam::Vec2),
    ClipboardFailed(String),
    /// A wireframe, for the sketch and revision it was recognised from.
    Recognized {
        sketch: uuid::Uuid,
        revision: u64,
        wire: std::sync::Arc<nc_core::recognize::Wireframe>,
    },
    /// Export finished: the files' shared path without extension, or what went wrong.
    Exported(Result<std::path::PathBuf, String>),
    /// The desktop's colour scheme: true for dark.
    ColorScheme(bool),
    Projects(Result<Vec<nc_store::ProjectInfo>, String>),
    SketchOpened {
        sketch: uuid::Uuid,
        result: Result<nc_core::Document, String>,
    },
    SketchCreated(Result<uuid::Uuid, String>),
    /// Another sketch, loaded for its sidebar thumbnail.
    Thumbnail {
        sketch: uuid::Uuid,
        doc: nc_core::Document,
    },
    /// The screen reader asked for something.
    Access(accesskit_winit::Event),
    /// A retry, or a newly chosen folder, for a store that would not open. `result` is `None` when the
    /// folder chooser was cancelled; `dir` is the chosen folder.
    StoreOpened {
        result: Option<Result<Box<Opened>, String>>,
        dir: Option<PathBuf>,
    },
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Access(event)
    }
}

/// Opens the store (in `dir`, else the XDG data directory) and the most recent sketch. A damaged
/// sketch opens read-only with what can be read; anything else that stops us saving is an error, and
/// the app blocks drawing until a retry or another folder works (Mac bug #4).
pub fn open(proxy: EventLoopProxy<UserEvent>, dir: Option<&Path>) -> Result<Opened, StoreError> {
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => dirs::data_dir()
            .ok_or_else(|| {
                StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no home or XDG data directory",
                ))
            })?
            .join("neuralcanvas"),
    };
    let path = dir.join("neuralcanvas.db");
    let store = Store::open(path, move |event| {
        let _ = proxy.send_event(UserEvent::Store(event));
    })?;
    let sketch = store.call(|db| db.last_or_new_sketch())?;
    match store.call(move |db| db.load(sketch)) {
        Ok(doc) => Ok((store, Session::new(sketch, doc), false)),
        Err(e @ StoreError::Corrupt { .. }) => {
            eprintln!("neuralcanvas: opening read-only: {e}");
            let doc = store.call(move |db| db.load_readable(sketch))?;
            Ok((store, Session::new(sketch, doc), true))
        }
        Err(e) => Err(e),
    }
}

/// Process start, for the cold-start measurement in profile builds.
#[cfg(feature = "profile")]
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    #[cfg(feature = "profile")]
    {
        STARTED.get_or_init(std::time::Instant::now);
        tracy_client::Client::start();
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("event loop");
    let (settings, store_dir) = app::settings::load();
    let opened = open(event_loop.create_proxy(), store_dir.as_deref()).map_err(|e| {
        eprintln!("neuralcanvas: cannot open the sketch store, nothing would be saved: {e}");
        e.to_string()
    });
    let signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ])
    .expect("install signal handlers");
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        let mut signals = signals;
        if signals.forever().next().is_some() {
            let _ = proxy.send_event(UserEvent::Quit);
        }
    });
    let proxy = event_loop.create_proxy();
    // Kept alive for the whole run: dropping it stops the updates. "No preference" counts as dark,
    // the app's default (DESIGN.md).
    let _preferences = mundy::Preferences::subscribe(
        mundy::Interest::ReducedMotion | mundy::Interest::ColorScheme,
        move |p| {
            let _ = proxy.send_event(UserEvent::ReducedMotion(p.reduced_motion.is_reduce()));
            let dark = p.color_scheme != mundy::ColorScheme::Light;
            let _ = proxy.send_event(UserEvent::ColorScheme(dark));
        },
    );
    let mut app = App::new(opened, event_loop.create_proxy(), settings, store_dir);
    event_loop.run_app(&mut app).expect("run app");
}
