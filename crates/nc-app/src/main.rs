//! NeuralCanvas: winit event loop + wgpu surface hosting the nc-render canvas. Frames are drawn only
//! when something changed (idle = 0% CPU, docs/PERFORMANCE-BUDGET.md). Every edit is a command on the
//! open sketch's session and goes to the store thread; this thread never touches disk.

mod app;
mod chrome;
mod clipboard;
mod gpu;
mod workers;

use app::App;
use nc_core::{Document, Session};
use nc_store::{Store, StoreError, StoreEvent};
use winit::event_loop::{EventLoop, EventLoopProxy};

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
}

/// Opens the store and the most recent sketch. A damaged sketch opens read-only and empty; anything
/// else that stops us saving is fatal, before any drawing can happen (Mac bug #4).
fn open(proxy: EventLoopProxy<UserEvent>) -> Result<(Store, Session, bool), StoreError> {
    let path = dirs::data_dir()
        .ok_or_else(|| {
            StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no home or XDG data directory",
            ))
        })?
        .join("neuralcanvas")
        .join("neuralcanvas.db");
    let store = Store::open(path, move |event| {
        let _ = proxy.send_event(UserEvent::Store(event));
    })?;
    let sketch = store.call(|db| db.last_or_new_sketch())?;
    match store.call(move |db| db.load(sketch)) {
        Ok(doc) => Ok((store, Session::new(sketch, doc), false)),
        // shortcut: opens empty instead of showing the readable rows; a partial load needs the Phase 5
        // UI to explain what is missing.
        Err(e @ StoreError::Corrupt { .. }) => {
            eprintln!("neuralcanvas: opening read-only: {e}");
            Ok((store, Session::new(sketch, Document::default()), true))
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
    let (store, session, read_only) = match open(event_loop.create_proxy()) {
        Ok(opened) => opened,
        // shortcut: stderr and exit, before any window; the Phase 5 shell turns this into the blocking
        // Retry / Choose location / Quit dialog (docs/PLAN.md §4, bug #4).
        Err(e) => {
            eprintln!("neuralcanvas: cannot open the sketch store, nothing would be saved: {e}");
            std::process::exit(1);
        }
    };
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
    // Kept alive for the whole run: dropping it stops the updates.
    let _reduced_motion = mundy::Preferences::subscribe(mundy::Interest::ReducedMotion, move |p| {
        let _ = proxy.send_event(UserEvent::ReducedMotion(p.reduced_motion.is_reduce()));
    });
    let mut app = App::new(store, session, read_only, event_loop.create_proxy());
    event_loop.run_app(&mut app).expect("run app");
}
