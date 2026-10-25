//! Copy and paste on a worker thread: building SVG and PNG for other apps is export work, and reading
//! the clipboard blocks on the app that owns it. Neither may stall the canvas.

use crate::UserEvent;
use glam::Vec2;
use nc_core::Element;
use nc_core::select::{CLIPBOARD_MIME, decode, encode};
use std::io::Read;
use std::sync::Arc;
use std::sync::mpsc;
use winit::event_loop::EventLoopProxy;
use wl_clipboard_rs::{copy, paste};

enum Request {
    /// Elements in z-order, and the canvas look: background and whether ink shows lightness-inverted.
    Copy(Vec<Arc<Element>>, [f32; 4], bool),
    Paste(Vec2),
}

pub struct Clipboard {
    tx: mpsc::Sender<Request>,
}

/// PNG pixels per world unit: crisp when pasted into documents at 2x.
const PNG_SCALE: f32 = 2.0;

impl Clipboard {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                crate::workers::idle_priority();
                for request in rx {
                    let event = match request {
                        Request::Copy(elements, background, light) => {
                            match copy_out(&elements, background, light) {
                                Ok(()) => continue,
                                Err(e) => UserEvent::ClipboardFailed(nc_shell::copy::copy_failed(
                                    &e.to_string(),
                                )),
                            }
                        }
                        Request::Paste(centre) => match paste_in() {
                            Ok(Some(elements)) => UserEvent::Pasted(elements, centre),
                            Ok(None) => continue,
                            Err(e) => UserEvent::ClipboardFailed(nc_shell::copy::paste_failed(
                                &e.to_string(),
                            )),
                        },
                    };
                    let _ = proxy.send_event(event);
                }
            })
            .expect("spawn clipboard thread");
        Self { tx }
    }

    /// `elements` in z-order; `background` and `light` give the pasted image the canvas's look.
    pub fn copy(&self, elements: Vec<Arc<Element>>, background: [f32; 4], light: bool) {
        let _ = self.tx.send(Request::Copy(elements, background, light));
    }

    /// Pasted elements arrive as [`UserEvent::Pasted`], to be centred on `centre` (world units).
    pub fn paste(&self, centre: Vec2) {
        let _ = self.tx.send(Request::Paste(centre));
    }
}

/// Offers the internal format (lossless, for pasting back into NeuralCanvas) plus SVG and PNG.
fn copy_out(
    elements: &[Arc<Element>],
    background: [f32; 4],
    light: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut sources = vec![copy::MimeSource {
        source: copy::Source::Bytes(encode(elements).into()),
        mime_type: copy::MimeType::Specific(CLIPBOARD_MIME.into()),
    }];
    if let Some(svg) = nc_export::svg(elements, Some(background), light) {
        let png = nc_export::png(&svg, PNG_SCALE)?;
        sources.push(copy::MimeSource {
            source: copy::Source::Bytes(svg.into_bytes().into()),
            mime_type: copy::MimeType::Specific("image/svg+xml".into()),
        });
        sources.push(copy::MimeSource {
            source: copy::Source::Bytes(png.into()),
            mime_type: copy::MimeType::Specific("image/png".into()),
        });
    }
    // No text/plain alias of the SVG: text fields would otherwise paste markup instead of the image.
    let mut options = copy::Options::new();
    options.omit_additional_text_mime_types(true);
    options.copy_multi(sources)?;
    Ok(())
}

/// `Ok(None)` when the clipboard holds nothing of ours (an empty clipboard, text, another app's image).
fn paste_in() -> Result<Option<Vec<Element>>, Box<dyn std::error::Error>> {
    let (mut pipe, _) = match paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        paste::MimeType::Specific(CLIPBOARD_MIME),
    ) {
        Ok(found) => found,
        Err(paste::Error::NoSeats | paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(decode(&bytes))
}
