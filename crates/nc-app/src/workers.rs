//! Recognition and export, each on its own thread: neither may ever stall the canvas. Results come back
//! as user events; recognition results carry the sketch and revision they were computed for, so the app
//! can drop stale ones.

use crate::UserEvent;
use crossbeam_channel::{Sender, unbounded};
use nc_core::Element;
use nc_core::recognize::{Wireframe, recognize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;
use winit::event_loop::EventLoopProxy;

/// Puts the calling worker thread on idle priority: it gets only CPU time nothing else wants, so
/// recognition and export never compete with the canvas, even on a busy machine.
pub fn idle_priority() {
    // SAFETY: a plain syscall on the calling thread with a valid parameter.
    let failed = unsafe {
        let param = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(0, libc::SCHED_IDLE, &param) != 0
    };
    if failed {
        eprintln!(
            "neuralcanvas: could not lower a worker's priority: {}",
            std::io::Error::last_os_error()
        );
    }
}

struct Job {
    sketch: Uuid,
    revision: u64,
    elements: Vec<Arc<Element>>,
}

pub struct Recognizer {
    tx: Sender<Job>,
}

impl Recognizer {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = unbounded::<Job>();
        std::thread::Builder::new()
            .name("recognize".into())
            .spawn(move || {
                idle_priority();
                while let Ok(mut job) = rx.recv() {
                    // Edits arrive faster than recognition on big sketches: only the newest matters.
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    let wire = Arc::new(recognize(&job.elements));
                    let done = UserEvent::Recognized {
                        sketch: job.sketch,
                        revision: job.revision,
                        wire,
                    };
                    if proxy.send_event(done).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn recognition thread");
        Self { tx }
    }

    /// Recognises a snapshot of the sketch at `revision`.
    pub fn request(&self, sketch: Uuid, revision: u64, elements: Vec<Arc<Element>>) {
        let _ = self.tx.send(Job {
            sketch,
            revision,
            elements,
        });
    }
}

struct ExportJob {
    elements: Vec<Arc<Element>>,
    wireframe: bool,
}

pub struct Exporter {
    tx: Sender<ExportJob>,
}

/// Raster exports are 2 px per world unit.
const RASTER_SCALE: f32 = 2.0;

impl Exporter {
    /// `background` is the canvas colour: exports look like the canvas.
    pub fn new(proxy: EventLoopProxy<UserEvent>, background: [f32; 4]) -> Self {
        let (tx, rx) = unbounded::<ExportJob>();
        std::thread::Builder::new()
            .name("export".into())
            .spawn(move || {
                idle_priority();
                for job in rx {
                    let result = export(&job, background).map_err(|e| e.to_string());
                    if proxy.send_event(UserEvent::Exported(result)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn export thread");
        Self { tx }
    }

    pub fn export(&self, elements: Vec<Arc<Element>>, wireframe: bool) {
        let _ = self.tx.send(ExportJob {
            elements,
            wireframe,
        });
    }
}

/// `~/Pictures/NeuralCanvas` (the XDG pictures directory), or the home directory without one.
fn export_dir() -> Option<PathBuf> {
    Some(
        dirs::picture_dir()
            .or_else(dirs::home_dir)?
            .join("NeuralCanvas"),
    )
}

/// The first `<dir>/<stem> N` with none of the four files taken, so nothing is overwritten.
fn free_base(dir: &Path, stem: &str) -> PathBuf {
    (1..)
        .map(|n| dir.join(format!("{stem} {n}")))
        .find(|base| {
            ["svg", "pdf", "png", "jpg"]
                .iter()
                .all(|ext| !base.with_extension(ext).exists())
        })
        .expect("some number is free")
}

/// Writes SVG, PDF, PNG and JPEG; returns the shared path without extension.
fn export(job: &ExportJob, background: [f32; 4]) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let svg = if job.wireframe {
        let wire: Wireframe = recognize(&job.elements);
        nc_export::wireframe_svg(&wire, Some(background))
    } else {
        nc_export::svg(&job.elements, Some(background))
    }
    .ok_or("the sketch is empty")?;
    let dir = export_dir().ok_or("no home directory to export to")?;
    std::fs::create_dir_all(&dir)?;
    let base = free_base(&dir, if job.wireframe { "Wireframe" } else { "Sketch" });
    std::fs::write(base.with_extension("svg"), &svg)?;
    std::fs::write(base.with_extension("pdf"), nc_export::pdf(&svg)?)?;
    std::fs::write(
        base.with_extension("png"),
        nc_export::png(&svg, RASTER_SCALE)?,
    )?;
    std::fs::write(
        base.with_extension("jpg"),
        nc_export::jpeg(&svg, RASTER_SCALE)?,
    )?;
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_names_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("nc-export-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_base(&dir, "Sketch"), dir.join("Sketch 1"));
        std::fs::write(dir.join("Sketch 1.pdf"), b"").unwrap();
        assert_eq!(
            free_base(&dir, "Sketch"),
            dir.join("Sketch 2"),
            "any one format taken"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
