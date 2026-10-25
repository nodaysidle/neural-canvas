//! Preferences on disk: `$XDG_CONFIG_HOME/neuralcanvas/settings.json`. Read once at startup; written
//! on a worker thread (the UI thread never touches disk), atomically (temp file, then rename). Besides
//! the user's preferences it remembers a sketch-store folder chosen after the default one failed.

use crossbeam_channel::{Sender, unbounded};
use nc_shell::{PressureCurve, Settings, ThemeChoice};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Stored {
    theme: String,
    grid: bool,
    smoothing: bool,
    pressure: String,
    #[serde(default)]
    sidebar: bool,
    #[serde(default)]
    inspector: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    store_dir: Option<PathBuf>,
}

impl Stored {
    fn new(s: Settings, store_dir: Option<PathBuf>) -> Self {
        Self {
            store_dir,
            theme: match s.theme {
                ThemeChoice::System => "system",
                ThemeChoice::Dark => "dark",
                ThemeChoice::Light => "light",
            }
            .into(),
            grid: s.grid,
            smoothing: s.smoothing,
            sidebar: s.sidebar,
            inspector: s.inspector,
            pressure: match s.pressure {
                PressureCurve::Soft => "soft",
                PressureCurve::Linear => "linear",
                PressureCurve::Firm => "firm",
            }
            .into(),
        }
    }
}

impl From<Stored> for Settings {
    /// Unknown values fall back to the defaults, so an older or hand-edited file still opens.
    fn from(s: Stored) -> Self {
        let d = Settings::default();
        Self {
            theme: match s.theme.as_str() {
                "dark" => ThemeChoice::Dark,
                "light" => ThemeChoice::Light,
                _ => d.theme,
            },
            grid: s.grid,
            smoothing: s.smoothing,
            sidebar: s.sidebar,
            inspector: s.inspector,
            pressure: match s.pressure.as_str() {
                "soft" => PressureCurve::Soft,
                "firm" => PressureCurve::Firm,
                _ => d.pressure,
            },
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("neuralcanvas")
            .join("settings.json"),
    )
}

/// The saved preferences and chosen store folder, or the defaults when there are none. A damaged
/// file is reported and ignored: preferences are never worth refusing to start over.
pub fn load() -> (Settings, Option<PathBuf>) {
    let Some(path) = path() else {
        return Default::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Stored>(&bytes) {
            Ok(mut stored) => {
                let dir = stored.store_dir.take();
                (stored.into(), dir)
            }
            Err(e) => {
                eprintln!(
                    "neuralcanvas: ignoring unreadable preferences {}: {e}",
                    path.display()
                );
                Default::default()
            }
        },
        Err(_) => Default::default(),
    }
}

enum Change {
    Settings(Settings),
    StoreDir(PathBuf),
}

pub struct SettingsWriter {
    tx: Sender<Change>,
}

impl SettingsWriter {
    /// `settings` and `store_dir` are what is on disk now; each change rewrites the whole file.
    pub fn new(mut settings: Settings, mut store_dir: Option<PathBuf>) -> Self {
        let (tx, rx) = unbounded::<Change>();
        std::thread::Builder::new()
            .name("settings".into())
            .spawn(move || {
                crate::workers::idle_priority();
                while let Ok(change) = rx.recv() {
                    for change in std::iter::once(change).chain(rx.try_iter()) {
                        match change {
                            Change::Settings(s) => settings = s,
                            Change::StoreDir(d) => store_dir = Some(d),
                        }
                    }
                    if let Err(e) = write(settings, store_dir.as_deref()) {
                        eprintln!("neuralcanvas: could not save preferences: {e}");
                    }
                }
            })
            .expect("spawn settings thread");
        Self { tx }
    }

    pub fn save(&self, settings: Settings) {
        let _ = self.tx.send(Change::Settings(settings));
    }

    /// Remembers the folder the sketch store now lives in.
    pub fn save_store_dir(&self, dir: PathBuf) {
        let _ = self.tx.send(Change::StoreDir(dir));
    }
}

fn write(settings: Settings, store_dir: Option<&Path>) -> std::io::Result<()> {
    let Some(path) = path() else { return Ok(()) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let stored = Stored::new(settings, store_dir.map(Path::to_path_buf));
    let json = serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_unknown_values() {
        let s = Settings {
            theme: ThemeChoice::Light,
            grid: false,
            smoothing: false,
            pressure: PressureCurve::Firm,
            sidebar: true,
            inspector: false,
        };
        let json = serde_json::to_string(&Stored::new(s, Some("/data/sketches".into()))).unwrap();
        let mut stored = serde_json::from_str::<Stored>(&json).unwrap();
        assert_eq!(
            stored.store_dir.take(),
            Some(PathBuf::from("/data/sketches"))
        );
        let back: Settings = stored.into();
        assert_eq!(back, s);
        let odd: Settings = serde_json::from_str::<Stored>(
            r#"{"theme":"sepia","grid":true,"smoothing":true,"pressure":"heavy"}"#,
        )
        .unwrap()
        .into();
        assert_eq!(odd, Settings::default());
    }
}
