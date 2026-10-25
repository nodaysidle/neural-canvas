//! The store thread. The UI sends snapshots and never touches disk; the thread writes at most
//! [`DEBOUNCE`] after the first unsaved change, and immediately on [`Store::flush`] (Mac bug #5).

use crate::db::{Db, Result, StoreError};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use nc_core::Element;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Longest a change waits before it is written. Measured from the first unsaved change, not the last,
/// so continuous drawing still saves every 300 ms.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// How soon a failed write is retried.
pub const RETRY: Duration = Duration::from_secs(1);

/// Reported to the UI through the `on_event` callback, on the store thread.
#[derive(Clone, Debug, PartialEq)]
pub enum StoreEvent {
    Saved {
        sketch: Uuid,
        revision: u64,
    },
    /// The change is kept in memory and retried; the UI must show this.
    SaveFailed {
        sketch: Uuid,
        message: String,
    },
}

type Call = Box<dyn FnOnce(&mut Db) + Send>;

enum Request {
    Save {
        sketch: Uuid,
        revision: u64,
        elements: Vec<Arc<Element>>,
    },
    Flush(Sender<Result<()>>),
    Call(Call),
}

pub struct Store {
    tx: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl Store {
    /// Starts the store thread and opens the database on it. Blocks until the database is open, so
    /// an unopenable store is an error here, never a silent in-memory fallback (Mac bug #4).
    pub fn open(path: PathBuf, on_event: impl Fn(StoreEvent) + Send + 'static) -> Result<Self> {
        let (tx, rx) = unbounded();
        let (ready_tx, ready_rx) = bounded(1);
        let thread = std::thread::Builder::new()
            .name("nc-store".into())
            .spawn(move || match Db::open(&path) {
                Ok(db) => {
                    let _ = ready_tx.send(Ok(()));
                    run(db, rx, on_event);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })?;
        ready_rx.recv().map_err(|_| StoreError::Closed)??;
        Ok(Self {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    /// Queues the sketch's current content. Never blocks.
    pub fn save(&self, sketch: Uuid, revision: u64, elements: Vec<Arc<Element>>) {
        self.send(Request::Save {
            sketch,
            revision,
            elements,
        });
    }

    /// Writes everything queued now and waits for the result. For sketch switch and quit.
    pub fn flush(&self) -> Result<()> {
        let (reply, result) = bounded(1);
        self.send(Request::Flush(reply));
        result.recv().map_err(|_| StoreError::Closed)?
    }

    /// Runs `f` on the store thread after writing everything queued, then hands its result to `done`
    /// there, without waiting: what the UI thread uses (sidebar, sketch switching). `done` typically
    /// posts the result back to the event loop.
    pub fn request<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> Result<T> + Send + 'static,
        done: impl FnOnce(Result<T>) + Send + 'static,
    ) {
        self.send(Request::Call(Box::new(move |db| done(f(db)))));
    }

    /// Runs `f` on the store thread after writing everything queued, and waits for its result.
    /// For loads and project/sketch management: rare, user-initiated, never per frame.
    pub fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, result) = bounded(1);
        self.send(Request::Call(Box::new(move |db| {
            let _ = reply.send(f(db));
        })));
        result.recv().map_err(|_| StoreError::Closed)?
    }

    fn send(&self, request: Request) {
        if let Some(tx) = &self.tx {
            // A send only fails if the thread is gone; callers see `Closed` on their reply channel.
            let _ = tx.send(request);
        }
    }
}

impl Drop for Store {
    /// Closing the channel makes the thread write what is queued and exit.
    fn drop(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(mut db: Db, rx: Receiver<Request>, on_event: impl Fn(StoreEvent)) {
    // Latest unsaved content per sketch; a newer save replaces an older one.
    let mut pending: HashMap<Uuid, (u64, Vec<Arc<Element>>)> = HashMap::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let request = match deadline {
            Some(at) => rx.recv_deadline(at),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match request {
            Ok(Request::Save {
                sketch,
                revision,
                elements,
            }) => {
                pending.insert(sketch, (revision, elements));
                deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            Ok(Request::Flush(reply)) => {
                let result = write(&mut db, &mut pending, &on_event);
                deadline = result.is_err().then(|| Instant::now() + RETRY);
                let _ = reply.send(result);
            }
            Ok(Request::Call(f)) => {
                // A failed write is already reported; the call still runs against what is on disk.
                if write(&mut db, &mut pending, &on_event).is_err() {
                    deadline = Some(Instant::now() + RETRY);
                }
                f(&mut db);
            }
            Err(RecvTimeoutError::Timeout) => {
                deadline = write(&mut db, &mut pending, &on_event)
                    .is_err()
                    .then(|| Instant::now() + RETRY);
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = write(&mut db, &mut pending, &on_event);
                return;
            }
        }
    }
}

/// Writes every pending sketch. Failed sketches stay pending; the first error is returned.
fn write(
    db: &mut Db,
    pending: &mut HashMap<Uuid, (u64, Vec<Arc<Element>>)>,
    on_event: &impl Fn(StoreEvent),
) -> Result<()> {
    let mut first_error = None;
    for (sketch, (revision, elements)) in std::mem::take(pending) {
        match db.save(sketch, revision, &elements) {
            Ok(()) => on_event(StoreEvent::Saved { sketch, revision }),
            Err(e) => {
                on_event(StoreEvent::SaveFailed {
                    sketch,
                    message: e.to_string(),
                });
                pending.insert(sketch, (revision, elements));
                first_error.get_or_insert(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
