//! Recognition, export and the style mirror, each on its own thread: none may ever stall the canvas.
//! Results come back as user events; recognition results carry the sketch and revision they were
//! computed for, so the app can drop stale ones.

use crate::UserEvent;
use ashpd::desktop::file_chooser::{FileFilter, OpenFileRequest, SelectedFiles};
use crossbeam_channel::{Sender, unbounded};
use nc_core::Element;
use nc_core::recognize::draw::Look;
use nc_core::recognize::{Wireframe, recognize};
use nc_core::style::{Extracted, Style};
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
    /// The sketch's name: files are called after it.
    name: String,
    wireframe: bool,
    /// The canvas look: exports look like the canvas.
    background: [f32; 4],
    light: bool,
    /// The wireframe's style, if one is applied.
    style: Option<Style>,
}

pub struct Exporter {
    tx: Sender<ExportJob>,
}

/// Raster exports are 2 px per world unit.
const RASTER_SCALE: f32 = 2.0;

impl Exporter {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = unbounded::<ExportJob>();
        std::thread::Builder::new()
            .name("export".into())
            .spawn(move || {
                idle_priority();
                for job in rx {
                    let result = export(&job).map_err(|e| e.to_string());
                    if proxy.send_event(UserEvent::Exported(result)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn export thread");
        Self { tx }
    }

    pub fn export(
        &self,
        elements: Vec<Arc<Element>>,
        name: &str,
        wireframe: bool,
        background: [f32; 4],
        light: bool,
        style: Option<Style>,
    ) {
        let _ = self.tx.send(ExportJob {
            elements,
            name: name.into(),
            wireframe,
            background,
            light,
            style,
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

/// A file name stem from a sketch name: no path separators, never empty.
fn stem(name: &str, wireframe: bool) -> String {
    let name = name.replace(['/', '\\', '\0'], "-");
    let name = name.trim();
    let name = if name.is_empty() { "Sketch" } else { name };
    if wireframe {
        format!("{name} wireframe")
    } else {
        name.into()
    }
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
fn export(job: &ExportJob) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let (background, light) = (job.background, job.light);
    let svg = if job.wireframe {
        let wire: Wireframe = recognize(&job.elements);
        let look = job.style.map_or(Look::Plain { light }, Look::Styled);
        nc_export::wireframe_svg(&wire, Some(background), &look)
    } else {
        nc_export::svg(&job.elements, Some(background), light)
    }
    .ok_or("the sketch is empty")?;
    let dir = export_dir().ok_or("no home directory to export to")?;
    std::fs::create_dir_all(&dir)?;
    let base = free_base(&dir, &stem(&job.name, job.wireframe));
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

/// Asks the desktop's file chooser (the XDG portal) for a folder. `Ok(None)`: the person cancelled.
/// Blocks, so call it on a worker.
pub fn pick_folder(title: &str) -> Result<Option<PathBuf>, String> {
    pick(SelectedFiles::open_file().title(title).directory(true))
}

/// Asks the file chooser for a PNG or JPEG image. Blocks, so call it on a worker.
pub fn pick_image(title: &str) -> Result<Option<PathBuf>, String> {
    pick(
        SelectedFiles::open_file().title(title).filter(
            FileFilter::new(nc_shell::copy::IMAGE_FILTER)
                .mimetype("image/png")
                .mimetype("image/jpeg"),
        ),
    )
}

fn pick(request: OpenFileRequest) -> Result<Option<PathBuf>, String> {
    use ashpd::desktop::ResponseError;
    async_io::block_on(async {
        let request = request
            .modal(true)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        match request.response() {
            Ok(files) => Ok(files.uris().first().and_then(|u| file_path(u.as_str()))),
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    })
}

/// Where a screenshot to take a style from comes from.
pub enum Screenshot {
    /// Ask the file chooser for one.
    Pick,
    /// Image bytes pasted from the clipboard.
    Pasted(Vec<u8>),
}

/// Reads styles off screenshots: decoding and k-means run here, never on the UI thread.
pub struct Styler {
    tx: Sender<Screenshot>,
}

impl Styler {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = unbounded::<Screenshot>();
        std::thread::Builder::new()
            .name("style".into())
            .spawn(move || {
                idle_priority();
                for job in rx {
                    let (bytes, name) = match job {
                        Screenshot::Pick => match pick_image(nc_shell::copy::IMPORT_STYLE_TITLE) {
                            Ok(None) => continue,
                            Ok(Some(path)) => match std::fs::read(&path) {
                                Ok(bytes) => (bytes, screenshot_name(&path)),
                                Err(e) => {
                                    let _ =
                                        proxy.send_event(UserEvent::StyleRead(Err(e.to_string())));
                                    continue;
                                }
                            },
                            Err(e) => {
                                let _ = proxy.send_event(UserEvent::StyleRead(Err(e)));
                                continue;
                            }
                        },
                        Screenshot::Pasted(bytes) => {
                            (bytes, nc_shell::copy::PASTED_SCREENSHOT.into())
                        }
                    };
                    let _ = proxy.send_event(UserEvent::StyleReading);
                    let result = read_style(&bytes).map(|e| (Box::new(e), name));
                    if proxy.send_event(UserEvent::StyleRead(result)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn style thread");
        Self { tx }
    }

    pub fn read(&self, from: Screenshot) {
        let _ = self.tx.send(from);
    }
}

/// A preset's name from the screenshot's file name.
fn screenshot_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| nc_shell::copy::PASTED_SCREENSHOT.into())
}

/// Largest screenshot side we decode, px: a 8K screen and then some.
const MAX_SIDE: u32 = 16_384;

/// Decodes a PNG or JPEG (format sniffed from its bytes, size limited) and reads its style.
fn read_style(bytes: &[u8]) -> Result<Extracted, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| e.to_string())?.to_rgba8();
    nc_core::style::extract(image.as_raw(), image.width(), image.height())
        .ok_or_else(|| "the image is empty".into())
}

/// The local path of a `file://` URI, percent-decoded byte for byte (paths need not be UTF-8).
fn file_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let path = uri.strip_prefix("file://")?;
    let bytes: Vec<u8> = percent_encoding::percent_decode_str(path).collect();
    Some(std::ffi::OsString::from_vec(bytes).into())
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

    #[test]
    fn export_stems_come_from_the_sketch_name_and_stay_in_the_folder() {
        assert_eq!(stem("Login flow", false), "Login flow");
        assert_eq!(stem("Login flow", true), "Login flow wireframe");
        assert_eq!(stem("../a/b", false), "..-a-b");
        assert_eq!(stem("  ", false), "Sketch");
    }

    #[test]
    fn screenshots_decode_off_bytes_and_bad_ones_say_why() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(32, 32, image::Rgba([16, 18, 24, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(read_style(&png).unwrap().style.background, 0x101218);
        assert!(read_style(b"not an image").is_err());
        assert_eq!(
            screenshot_name(Path::new("/tmp/Files app.png")),
            "Files app"
        );
    }

    #[test]
    fn portal_uris_become_local_paths() {
        assert_eq!(
            file_path("file:///home/a/My%20Sketches"),
            Some(PathBuf::from("/home/a/My Sketches"))
        );
        assert_eq!(file_path("https://example.com/x"), None);
    }
}
