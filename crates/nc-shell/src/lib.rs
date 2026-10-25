//! The application shell, drawn in our own wgpu chrome layer (`nc_render::ui`): top bar, floating tool
//! palette, sidebar, inspector, zoom readout, empty-canvas hint, banner, toasts, shortcuts sheet and
//! preferences. Pure state and layout: [`Shell::build`] turns the app's state into a `UiScene` plus
//! hit regions (which also give the keyboard focus order and the accessibility tree), so it is
//! unit-tested here and drawn as-is by the app and by the bench.

pub mod copy;
pub mod theme;

use glam::Vec2;
use nc_core::recognize::QualityStatus;
use nc_core::style::{Note, Style};
use nc_core::{Rect, ShapeType, Tool};
use nc_render::ui::{Font, Measure, UiImage, UiLayer, UiRect, UiScene, UiText, text_height};
use nc_store::{Preset, ProjectInfo};
use theme::Palette;
use uuid::Uuid;

pub type Bounds = [f32; 4];

/// Ink colours on the palette (linear RGB, as drawn on the dark canvas).
pub const INKS: [[f32; 4]; 5] = [
    [0.85, 0.87, 0.93, 1.0],
    [1.0, 0.24, 0.17, 1.0],
    [0.12, 0.43, 1.0, 1.0],
    [0.09, 0.64, 0.38, 1.0],
    [0.89, 0.53, 0.07, 1.0],
];
/// Stroke widths on the palette, world units.
pub const WIDTHS: [f32; 4] = [1.5, 2.5, 4.0, 8.0];

const BAR_H: f32 = 40.0;
const SIDE_W: f32 = 264.0;
const PALETTE_W: f32 = 132.0;
const PAD: f32 = 12.0;
const ROW: f32 = 28.0;
const BODY: f32 = 13.0;
const SMALL: f32 = 12.0;
const TITLE: f32 = 15.0;
const PANEL_RADIUS: f32 = 8.0;
const CONTROL_RADIUS: f32 = 6.0;
const THUMB: Vec2 = Vec2::new(72.0, 46.0);
const TOAST_SECS: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    SketchName(Uuid),
    ProjectName(Uuid),
    X,
    Y,
    W,
    H,
    StrokeWidth,
    Label,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PressureCurve {
    Soft,
    #[default]
    Linear,
    Firm,
}

impl PressureCurve {
    /// The pressure the ink uses for a raw stylus pressure.
    pub fn apply(self, p: f32) -> f32 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Self::Soft => p.powf(0.6),
            Self::Linear => p,
            Self::Firm => p.powf(1.6),
        }
    }
}

/// User preferences (persisted by the app).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub grid: bool,
    pub smoothing: bool,
    pub pressure: PressureCurve,
    /// The sidebar and inspector stay as the user left them (UX principle 1).
    pub sidebar: bool,
    pub inspector: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            grid: true,
            smoothing: true,
            pressure: PressureCurve::Linear,
            sidebar: false,
            inspector: false,
        }
    }
}

/// One choice in a preferences row: its label, what it sets, and whether it is the current one.
type PrefOption = (&'static str, Pref, bool);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pref {
    Theme(ThemeChoice),
    Grid(bool),
    Smoothing(bool),
    Pressure(PressureCurve),
}

/// What a click, or Enter on the focused control, does. The app performs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hit {
    Tool(Tool),
    Ink(usize),
    Width(usize),
    Sidebar,
    Inspector,
    Shortcuts,
    Preferences,
    /// 0 sketch, 1 hybrid, 2 wireframe, 3 side by side.
    Mode(u8),
    Review,
    Export,
    ExportWireframe,
    ZoomToFit,
    NewProject,
    NewSketch(Uuid),
    Sketch(Uuid),
    Field(Field),
    TypeMenu,
    TypeOption(ShapeType),
    ResetWireframe,
    Toast(u64, ToastAction),
    DismissToast(u64),
    Retry,
    /// The store dialog: pick another folder for the sketch store, or quit.
    ChooseLocation,
    Quit,
    Pref(Pref),
    /// The inspector's style section.
    ImportStyle,
    ApplyStyle,
    DiscardStyle,
    RemoveStyle,
    SavePreset,
    Preset(Uuid),
    DeletePreset(Uuid),
    CloseDialog,
    /// Over chrome that does nothing itself: keeps the click off the canvas.
    Panel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastAction {
    Retry,
    Review,
    OpenFolder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
    /// Work in progress: stays until the app replaces or dismisses it.
    Progress,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
    pub action: Option<ToastAction>,
    /// Info toasts leave by themselves at this time (seconds, app clock); errors stay.
    pub until: Option<f64>,
}

/// Accessibility role of a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Button,
    Toggle { on: bool },
    TextField,
    ListItem { selected: bool },
    Label,
    Group,
}

/// A clickable or labelled part of the chrome, in drawing order.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub bounds: Bounds,
    pub hit: Hit,
    pub label: String,
    pub role: Role,
    pub focusable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SaveStatus {
    Saved,
    Saving,
    Failed(String),
    ReadOnly,
}

/// What the inspector shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Inspect {
    Nothing,
    /// Selected strokes and shapes: their bounds, and their common width if they share one.
    Elements {
        count: usize,
        bounds: Rect,
        width: Option<f32>,
    },
    /// A wireframe element: recognised (or pinned by an earlier edit).
    Wire {
        shape_type: ShapeType,
        bounds: Rect,
        confidence: f32,
        uncertain: bool,
        label: String,
        pinned: bool,
    },
}

/// The app state the shell draws, rebuilt by the app whenever it changes.
#[derive(Clone, Debug)]
pub struct Status<'a> {
    pub dark: bool,
    pub tool: Tool,
    pub ink: usize,
    pub width: usize,
    pub zoom: f32,
    pub wire_level: f32,
    pub split: bool,
    pub projects: &'a [ProjectInfo],
    pub sketch: Uuid,
    pub save: SaveStatus,
    /// Shown while the wireframe is visible: its status and how many items are uncertain.
    pub quality: Option<(QualityStatus, usize)>,
    pub inspect: Inspect,
    pub empty: bool,
    /// A stroke or drag is in progress: the palette steps back.
    pub drawing: bool,
    pub summary: String,
    pub settings: Settings,
    pub loading: bool,
    /// The sketch store can't be opened: a blocking dialog covers everything (Mac bug #4).
    pub store_error: Option<&'a str>,
    pub style: StyleView<'a>,
}

/// The style mirror, as the inspector shows it.
#[derive(Clone, Copy, Debug, Default)]
pub struct StyleView<'a> {
    /// A screenshot is being read.
    pub reading: bool,
    /// Read off a screenshot and not applied yet, with what the screenshot didn't show.
    pub pending: Option<(&'a Style, &'a [Note])>,
    /// The style the sketch's wireframe is drawn in.
    pub applied: Option<&'a Style>,
    pub presets: &'a [Preset],
}

/// A text field being typed into.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub field: Field,
    pub text: String,
    /// Byte index of the caret.
    pub caret: usize,
}

/// Keys a text field understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialog {
    #[default]
    None,
    Shortcuts,
    Preferences,
}

/// The image id a sketch's thumbnail is registered under.
pub fn thumbnail_id(sketch: Uuid) -> u64 {
    sketch.as_u64_pair().0 ^ sketch.as_u64_pair().1
}

#[derive(Default)]
pub struct Shell {
    pub sidebar: bool,
    pub inspector: bool,
    pub dialog: Dialog,
    pub type_menu: bool,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    /// Keyboard focus; `None` means the canvas has it.
    pub focus: Option<Hit>,
    pub edit: Option<Edit>,
    pub toasts: Vec<Toast>,
    next_toast: u64,
    /// Sidebar scroll, logical px.
    pub scroll: f32,
    scene: UiScene,
    regions: Vec<Region>,
    /// Index of an open dialog's scrim in `regions`: keyboard focus stays in the regions after it.
    modal: Option<usize>,
}

struct Ctx<'a> {
    p: Palette,
    measure: &'a mut dyn Measure,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    focus: Option<Hit>,
    regions: &'a mut Vec<Region>,
    modal: Option<usize>,
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    [x, y, w, h]
}

fn contains(b: Bounds, p: Vec2) -> bool {
    p.x >= b[0] && p.y >= b[1] && p.x < b[0] + b[2] && p.y < b[1] + b[3]
}

fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

impl Ctx<'_> {
    fn panel(&self, layer: &mut UiLayer, b: Bounds, radius: f32) {
        layer.rects.push(UiRect {
            rect: b,
            fill: self.p.panel,
            border: self.p.border,
            radius,
            border_width: 1.0,
        });
    }

    fn text(&self, layer: &mut UiLayer, s: &str, at: Vec2, size: f32, font: Font, color: [f32; 4]) {
        layer.texts.push(UiText {
            text: s.into(),
            at: [at.x, at.y],
            size,
            font,
            color,
            width: None,
        });
    }

    fn region(
        &mut self,
        b: Bounds,
        hit: Hit,
        label: impl Into<String>,
        role: Role,
        focusable: bool,
    ) {
        self.regions.push(Region {
            bounds: b,
            hit,
            label: label.into(),
            role,
            focusable,
        });
    }

    /// A text button: background by state, accent fill when `active`, a focus ring when focused.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        layer: &mut UiLayer,
        b: Bounds,
        label: &str,
        hint: Option<&str>,
        hit: Hit,
        active: bool,
        role: Role,
    ) {
        let p = self.p;
        let fill = if active {
            p.accent
        } else if self.pressed == Some(hit) {
            p.pressed
        } else if self.hover == Some(hit) {
            p.raised
        } else {
            [0.0; 4]
        };
        let focused = self.focus == Some(hit);
        layer.rects.push(UiRect {
            rect: b,
            fill,
            border: p.accent,
            radius: CONTROL_RADIUS,
            border_width: if focused { 2.0 } else { 0.0 },
        });
        let color = if active { p.on_accent } else { p.text };
        let y = b[1] + (b[3] - text_height(BODY)) / 2.0;
        self.text(
            layer,
            label,
            Vec2::new(b[0] + 8.0, y),
            BODY,
            Font::Sans,
            color,
        );
        if let Some(h) = hint {
            let w = self.measure.width(h, SMALL, Font::Mono);
            let hint_color = if active { p.on_accent } else { p.muted };
            let hy = b[1] + (b[3] - text_height(SMALL)) / 2.0;
            self.text(
                layer,
                h,
                Vec2::new(b[0] + b[2] - 8.0 - w, hy),
                SMALL,
                Font::Mono,
                hint_color,
            );
        }
        let label = match hint {
            Some(h) => format!("{label} ({h})"),
            None => label.into(),
        };
        self.region(b, hit, label, role, true);
    }

    /// A button sized to its label.
    fn text_button(
        &mut self,
        layer: &mut UiLayer,
        at: Vec2,
        label: &str,
        hit: Hit,
        active: bool,
    ) -> f32 {
        let w = self.measure.width(label, BODY, Font::Sans) + 16.0;
        self.button(
            layer,
            rect(at.x, at.y, w, ROW),
            label,
            None,
            hit,
            active,
            Role::Button,
        );
        w
    }
}

impl Shell {
    pub fn new() -> Self {
        Self {
            sidebar: false,
            inspector: false,
            ..Self::default()
        }
    }

    pub fn scene(&self) -> &UiScene {
        &self.scene
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// The layer under all chrome, for things that belong to the canvas (wireframe labels). The app
    /// fills it; `build` leaves it alone.
    pub fn canvas_layer(&mut self) -> &mut UiLayer {
        &mut self.scene.canvas
    }

    /// The chrome under `p`, topmost first (popups are added after the base layer).
    pub fn hit(&self, p: Vec2) -> Option<Hit> {
        self.regions
            .iter()
            .rev()
            .find(|r| contains(r.bounds, p))
            .map(|r| r.hit)
    }

    /// Whether the pointer at `p` is over chrome (so the canvas must not get the event).
    pub fn covers(&self, p: Vec2) -> bool {
        self.hit(p).is_some()
    }

    /// Hover tracking; true if the chrome needs redrawing.
    pub fn pointer_moved(&mut self, p: Vec2) -> bool {
        let hit = self.hit(p).filter(|h| *h != Hit::Panel);
        std::mem::replace(&mut self.hover, hit) != hit
    }

    /// Tab / Shift+Tab: moves keyboard focus through the controls in order, then back to the canvas.
    pub fn focus_next(&mut self, back: bool) {
        let order: Vec<Hit> = self.regions[self.modal.unwrap_or(0)..]
            .iter()
            .filter(|r| r.focusable)
            .map(|r| r.hit)
            .collect();
        if order.is_empty() {
            self.focus = None;
            return;
        }
        let at = self.focus.and_then(|f| order.iter().position(|h| *h == f));
        // In a dialog, focus cycles through its controls and never falls back to the canvas.
        if self.modal.is_some() {
            let n = order.len();
            let i = match (at, back) {
                (None, false) => 0,
                (None, true) => n - 1,
                (Some(i), false) => (i + 1) % n,
                (Some(i), true) => (i + n - 1) % n,
            };
            self.focus = Some(order[i]);
            return;
        }
        self.focus = match (at, back) {
            (None, false) => Some(order[0]),
            (None, true) => order.last().copied(),
            (Some(i), false) => order.get(i + 1).copied(),
            (Some(0), true) => None,
            (Some(i), true) => Some(order[i - 1]),
        };
    }

    pub fn toast(
        &mut self,
        kind: ToastKind,
        message: impl Into<String>,
        action: Option<ToastAction>,
        now: f64,
    ) -> u64 {
        self.next_toast += 1;
        let id = self.next_toast;
        let message = message.into();
        // The same message again replaces the old one rather than stacking.
        self.toasts.retain(|t| t.message != message);
        self.toasts.push(Toast {
            id,
            kind,
            message,
            action,
            until: (kind == ToastKind::Info).then_some(now + TOAST_SECS),
        });
        while self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
        id
    }

    pub fn dismiss(&mut self, id: u64) {
        self.toasts.retain(|t| t.id != id);
    }

    /// Drops expired toasts; returns when the next one expires, if any.
    pub fn tick(&mut self, now: f64) -> Option<f64> {
        self.toasts.retain(|t| t.until.is_none_or(|u| u > now));
        self.toasts.iter().filter_map(|t| t.until).reduce(f64::min)
    }

    pub fn begin_edit(&mut self, field: Field, text: String) {
        self.edit = Some(Edit {
            field,
            caret: text.len(),
            text,
        });
    }

    /// A key for the field being edited. Returns the field and its new text when Enter commits it.
    pub fn edit_key(&mut self, key: EditKey) -> Option<(Field, String)> {
        let e = self.edit.as_mut()?;
        let prev = |s: &str, i: usize| s[..i].char_indices().last().map_or(0, |(j, _)| j);
        let next = |s: &str, i: usize| s[i..].chars().next().map_or(i, |c| i + c.len_utf8());
        match key {
            EditKey::Char(c) if !c.is_control() => {
                e.text.insert(e.caret, c);
                e.caret += c.len_utf8();
            }
            EditKey::Char(_) => {}
            EditKey::Backspace if e.caret > 0 => {
                let at = prev(&e.text, e.caret);
                e.text.replace_range(at..e.caret, "");
                e.caret = at;
            }
            EditKey::Backspace => {}
            EditKey::Delete if e.caret < e.text.len() => {
                let to = next(&e.text, e.caret);
                e.text.replace_range(e.caret..to, "");
            }
            EditKey::Delete => {}
            EditKey::Left => e.caret = prev(&e.text, e.caret),
            EditKey::Right => e.caret = next(&e.text, e.caret),
            EditKey::Home => e.caret = 0,
            EditKey::End => e.caret = e.text.len(),
            EditKey::Escape => self.edit = None,
            EditKey::Enter => {
                let e = self.edit.take()?;
                return Some((e.field, e.text.trim().to_owned()));
            }
        }
        None
    }

    /// Lays out the chrome for a `size` (logical px) window.
    pub fn build(&mut self, status: &Status, size: Vec2, measure: &mut dyn Measure) {
        let mut scene = std::mem::take(&mut self.scene);
        scene.base.clear();
        scene.popup.clear();
        let mut regions = std::mem::take(&mut self.regions);
        regions.clear();
        let mut cx = Ctx {
            p: Palette::of(status.dark),
            measure,
            hover: self.hover,
            pressed: self.pressed,
            focus: self.focus,
            regions: &mut regions,
            modal: None,
        };
        let top = self.top_bar(&mut cx, &mut scene.base, status, size);
        let mut left = PAD;
        if self.sidebar {
            self.sidebar(&mut cx, &mut scene.base, status, top, size);
            left += SIDE_W;
        }
        let right = if self.inspector {
            self.inspector(
                &mut cx,
                &mut scene.base,
                &mut scene.popup,
                status,
                top,
                size,
            );
            SIDE_W
        } else {
            0.0
        };
        Self::palette(&mut cx, &mut scene.base, status, Vec2::new(left, top + PAD));
        Self::zoom_readout(&mut cx, &mut scene.base, status, size, right);
        if status.empty && !status.loading {
            let w = cx.measure.width(copy::EMPTY_HINT, BODY, Font::Sans);
            let at = Vec2::new((size.x - w) / 2.0, size.y / 2.0 - 10.0);
            cx.text(
                &mut scene.base,
                copy::EMPTY_HINT,
                at,
                BODY,
                Font::Sans,
                cx.p.muted,
            );
        }
        if status.loading {
            let w = cx.measure.width(copy::LOADING_SKETCH, BODY, Font::Sans);
            let at = Vec2::new((size.x - w) / 2.0, size.y / 2.0 - 10.0);
            cx.text(
                &mut scene.base,
                copy::LOADING_SKETCH,
                at,
                BODY,
                Font::Sans,
                cx.p.muted,
            );
        }
        self.toasts_(&mut cx, &mut scene.popup, size);
        if let Some(reason) = status.store_error {
            Self::store_dialog(&mut cx, &mut scene.popup, reason, size);
            self.modal = cx.modal;
            self.scene = scene;
            self.regions = regions;
            return;
        }
        if self.dialog == Dialog::None && !self.type_menu {
            self.tooltip(&mut cx, &mut scene.popup, size);
        }
        match self.dialog {
            Dialog::None => {}
            Dialog::Shortcuts => Self::shortcuts(&mut cx, &mut scene.popup, size),
            Dialog::Preferences => Self::preferences(&mut cx, &mut scene.popup, status, size),
        }
        self.modal = cx.modal;
        self.scene = scene;
        self.regions = regions;
    }

    /// The hovered control's tooltip: what it does and its key, below it (above, near the bottom).
    fn tooltip(&self, cx: &mut Ctx, layer: &mut UiLayer, size: Vec2) {
        let Some(hit) = self.hover else { return };
        let tip = match hit {
            Hit::Sidebar => copy::TIP_SKETCHES,
            Hit::Inspector => copy::TIP_INSPECTOR,
            Hit::Export => copy::TIP_EXPORT,
            Hit::Shortcuts => copy::TIP_SHORTCUTS,
            Hit::Mode(m) => copy::TIP_MODES[m.min(3) as usize],
            Hit::Review => copy::TIP_REVIEW,
            Hit::ZoomToFit => copy::TIP_ZOOM,
            _ => return,
        };
        let Some(r) = cx.regions.iter().find(|r| r.hit == hit).map(|r| r.bounds) else {
            return;
        };
        let p = cx.p;
        let w = cx.measure.width(tip, SMALL, Font::Sans) + 16.0;
        let h = 24.0;
        let x = (r[0] + r[2] / 2.0 - w / 2.0).clamp(PAD, (size.x - w - PAD).max(PAD));
        let below = r[1] + r[3] + 6.0;
        let y = if below + h > size.y - PAD {
            r[1] - h - 6.0
        } else {
            below
        };
        layer.rects.push(UiRect {
            rect: rect(x, y, w, h),
            fill: p.raised,
            border: p.border,
            radius: CONTROL_RADIUS,
            border_width: 1.0,
        });
        cx.text(
            layer,
            tip,
            Vec2::new(x + 8.0, y + (h - text_height(SMALL)) / 2.0),
            SMALL,
            Font::Sans,
            p.text,
        );
    }

    /// Returns the y where the content below the bar (and banner) starts.
    fn top_bar(&self, cx: &mut Ctx, layer: &mut UiLayer, s: &Status, size: Vec2) -> f32 {
        let p = cx.p;
        layer.rects.push(UiRect {
            rect: rect(0.0, 0.0, size.x, BAR_H),
            fill: p.panel,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        layer.rects.push(UiRect {
            rect: rect(0.0, BAR_H - 1.0, size.x, 1.0),
            fill: p.border,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        cx.region(
            rect(0.0, 0.0, size.x, BAR_H),
            Hit::Panel,
            "",
            Role::Group,
            false,
        );
        let y = (BAR_H - ROW) / 2.0;
        let mut x = PAD - 4.0;
        x += cx.text_button(
            layer,
            Vec2::new(x, y),
            copy::SKETCHES,
            Hit::Sidebar,
            self.sidebar,
        ) + 8.0;

        // Project / sketch name; the sketch name is a field (click to rename).
        let (project, sketch) = s
            .projects
            .iter()
            .find_map(|p| Some((p, p.sketches.iter().find(|k| k.id == s.sketch)?)))
            .map_or((copy::DEFAULT_PROJECT, copy::DEFAULT_SKETCH), |(p, k)| {
                (p.name.as_str(), k.name.as_str())
            });
        let pw = cx.measure.width(project, BODY, Font::Sans);
        let ty = (BAR_H - text_height(BODY)) / 2.0;
        cx.text(layer, project, Vec2::new(x, ty), BODY, Font::Sans, p.muted);
        x += pw + 6.0;
        cx.text(layer, "/", Vec2::new(x, ty), BODY, Font::Sans, p.muted);
        x += 12.0;
        let field = Field::SketchName(s.sketch);
        x += self.field(
            cx,
            layer,
            Vec2::new(x, y),
            200.0,
            sketch,
            field,
            Font::SansStrong,
        ) + 8.0;

        // Right side, right to left: shortcuts, inspector, export, save status, review.
        let mut r = size.x - PAD + 4.0;
        // `name` is what a screen reader says, when the visible label alone says too little.
        let mut right = |cx: &mut Ctx,
                         layer: &mut UiLayer,
                         label: &str,
                         name: Option<&str>,
                         hit: Hit,
                         active: bool| {
            let w = cx.measure.width(label, BODY, Font::Sans) + 16.0;
            r -= w;
            cx.button(
                layer,
                rect(r, y, w, ROW),
                label,
                None,
                hit,
                active,
                Role::Button,
            );
            if let (Some(name), Some(region)) = (name, cx.regions.last_mut()) {
                region.label = name.into();
            }
            r -= 4.0;
        };
        right(
            cx,
            layer,
            "?",
            Some(copy::SHORTCUTS),
            Hit::Shortcuts,
            self.dialog == Dialog::Shortcuts,
        );
        right(
            cx,
            layer,
            copy::INSPECTOR,
            None,
            Hit::Inspector,
            self.inspector,
        );
        right(cx, layer, copy::EXPORT, None, Hit::Export, false);
        let (save, color) = match &s.save {
            SaveStatus::Saved => (copy::SAVED, p.muted),
            SaveStatus::Saving => (copy::SAVING, p.muted),
            SaveStatus::Failed(_) => (copy::NOT_SAVED, p.error),
            SaveStatus::ReadOnly => (copy::READ_ONLY, p.warning),
        };
        let sw = cx.measure.width(save, BODY, Font::Sans);
        r -= sw + 8.0;
        cx.text(layer, save, Vec2::new(r, ty), BODY, Font::Sans, color);
        cx.region(rect(r, y, sw, ROW), Hit::Panel, save, Role::Label, false);
        if let Some((status, uncertain)) = s.quality {
            let label = if uncertain > 0 {
                copy::review_count(uncertain)
            } else {
                copy::quality(status).to_owned()
            };
            let w = cx.measure.width(&label, BODY, Font::Sans) + 16.0;
            r -= w + 8.0;
            if uncertain > 0 {
                cx.button(
                    layer,
                    rect(r, y, w, ROW),
                    &label,
                    None,
                    Hit::Review,
                    false,
                    Role::Button,
                );
                // State, not decoration: the review action is the one amber item in the bar.
                let last = layer.texts.len() - 1;
                layer.texts[last].color = p.warning;
            } else {
                cx.text(
                    layer,
                    &label,
                    Vec2::new(r + 8.0, ty),
                    BODY,
                    Font::Sans,
                    p.muted,
                );
            }
        }

        // Centre: the view modes, if there is room between the two sides.
        let labels = copy::MODES;
        let widths: Vec<f32> = labels
            .iter()
            .map(|l| cx.measure.width(l, BODY, Font::Sans) + 16.0)
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 4.0 * 3.0;
        let mut mx = (size.x - total) / 2.0;
        if mx > x && mx + total < r {
            let mode = if s.split {
                3
            } else if s.wire_level <= 0.0 {
                0
            } else if s.wire_level < 1.0 {
                1
            } else {
                2
            };
            for (i, (l, w)) in labels.iter().zip(&widths).enumerate() {
                let hit = Hit::Mode(i as u8);
                cx.button(
                    layer,
                    rect(mx, y, *w, ROW),
                    l,
                    None,
                    hit,
                    mode == i,
                    Role::Toggle { on: mode == i },
                );
                mx += w + 4.0;
            }
            if mode == 1 && s.wire_level != 0.5 {
                let o = copy::opacity(s.wire_level);
                cx.text(
                    layer,
                    &o,
                    Vec2::new(mx + 4.0, (BAR_H - text_height(SMALL)) / 2.0),
                    SMALL,
                    Font::Mono,
                    p.muted,
                );
            }
        }

        let mut below = BAR_H;
        let banner = match &s.save {
            SaveStatus::Failed(reason) => Some((copy::not_saved(reason), true)),
            SaveStatus::ReadOnly => Some((copy::READ_ONLY_BANNER.to_owned(), false)),
            _ => None,
        };
        if let Some((message, retry)) = banner {
            let h = 36.0;
            layer.rects.push(UiRect {
                rect: rect(0.0, BAR_H, size.x, h),
                fill: p.panel,
                border: if retry { p.error } else { p.warning },
                radius: 0.0,
                border_width: 1.0,
            });
            cx.region(
                rect(0.0, BAR_H, size.x, h),
                Hit::Panel,
                message.clone(),
                Role::Label,
                false,
            );
            let color = if retry { p.error } else { p.warning };
            cx.text(
                layer,
                &message,
                Vec2::new(PAD, BAR_H + (h - text_height(BODY)) / 2.0),
                BODY,
                Font::Sans,
                color,
            );
            if retry {
                let w = cx.measure.width(copy::RETRY, BODY, Font::Sans) + 16.0;
                cx.button(
                    layer,
                    rect(size.x - PAD - w, BAR_H + 4.0, w, ROW),
                    copy::RETRY,
                    None,
                    Hit::Retry,
                    false,
                    Role::Button,
                );
            }
            below += h;
        }
        below
    }

    /// A single-line text field showing `value`, or the edit in progress. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn field(
        &self,
        cx: &mut Ctx,
        layer: &mut UiLayer,
        at: Vec2,
        max_w: f32,
        value: &str,
        field: Field,
        font: Font,
    ) -> f32 {
        let p = cx.p;
        let hit = Hit::Field(field);
        let editing = self.edit.as_ref().filter(|e| e.field == field);
        let shown = editing.map_or(value, |e| e.text.as_str());
        // Names (sketch, project) read as titles until touched and size to their text; inspector
        // values always look editable and fill their column.
        let boxed = !matches!(field, Field::SketchName(_) | Field::ProjectName(_));
        let w = if boxed {
            max_w
        } else {
            (cx.measure.width(shown, BODY, font) + 16.0).clamp(48.0, max_w)
        };
        let b = rect(at.x, at.y, w, ROW);
        let hovered = cx.hover == Some(hit);
        let active = editing.is_some() || cx.focus == Some(hit);
        layer.rects.push(UiRect {
            rect: b,
            fill: if editing.is_some() || hovered {
                p.raised
            } else {
                [0.0; 4]
            },
            border: if active { p.accent } else { p.border },
            radius: CONTROL_RADIUS,
            border_width: if active {
                2.0
            } else if boxed {
                1.0
            } else {
                0.0
            },
        });
        let ty = at.y + (ROW - text_height(BODY)) / 2.0;
        layer.texts.push(UiText {
            text: shown.into(),
            at: [at.x + 8.0, ty],
            size: BODY,
            font,
            color: p.text,
            width: Some(w - 12.0),
        });
        if let Some(e) = editing {
            let caret_x = at.x + 8.0 + cx.measure.width(&e.text[..e.caret], BODY, font);
            layer.rects.push(UiRect {
                rect: rect(caret_x, ty + 1.0, 1.5, text_height(BODY) - 2.0),
                fill: p.accent,
                border: [0.0; 4],
                radius: 0.0,
                border_width: 0.0,
            });
        }
        cx.region(b, hit, value, Role::TextField, true);
        w
    }

    fn palette(cx: &mut Ctx, layer: &mut UiLayer, s: &Status, at: Vec2) {
        let p = cx.p;
        let rows = copy::TOOLS.len() as f32;
        let h = PAD + rows * (ROW + 2.0) + 12.0 + 16.0 + 24.0 + 16.0 + 24.0 + PAD;
        let b = rect(at.x, at.y, PALETTE_W, h);
        // Steps back while drawing so it never competes with the ink (UX principle 1).
        let fade = if s.drawing { 0.3 } else { 1.0 };
        let first_rect = layer.rects.len();
        let first_text = layer.texts.len();
        cx.panel(layer, b, PANEL_RADIUS);
        cx.region(b, Hit::Panel, "", Role::Group, false);
        let mut y = at.y + PAD - 4.0;
        for (tool, name, key) in copy::TOOLS {
            let active = s.tool == tool;
            cx.button(
                layer,
                rect(at.x + 6.0, y, PALETTE_W - 12.0, ROW),
                name,
                Some(key),
                Hit::Tool(tool),
                active,
                Role::Toggle { on: active },
            );
            y += ROW + 2.0;
        }
        y += 8.0;
        cx.text(
            layer,
            copy::COLOR,
            Vec2::new(at.x + PAD, y),
            SMALL,
            Font::Sans,
            p.muted,
        );
        y += 18.0;
        for (i, ink) in INKS.iter().enumerate() {
            let sb = rect(at.x + PAD + i as f32 * 22.0, y, 18.0, 18.0);
            let hit = Hit::Ink(i);
            let on = s.ink == i;
            // Swatches show the ink as it looks on this theme's canvas.
            let shown = if s.dark {
                *ink
            } else {
                nc_core::theme::ink_on_light(*ink)
            };
            layer.rects.push(UiRect {
                rect: sb,
                fill: shown,
                border: if on || cx.focus == Some(hit) {
                    p.accent
                } else {
                    p.border
                },
                radius: 4.0,
                border_width: if on || cx.focus == Some(hit) {
                    2.0
                } else {
                    1.0
                },
            });
            cx.region(sb, hit, copy::INK_NAMES[i], Role::Toggle { on }, true);
        }
        y += 26.0;
        cx.text(
            layer,
            copy::WIDTH,
            Vec2::new(at.x + PAD, y),
            SMALL,
            Font::Sans,
            p.muted,
        );
        y += 18.0;
        for (i, w) in WIDTHS.iter().enumerate() {
            let sb = rect(at.x + PAD + i as f32 * 27.0, y, 24.0, 24.0);
            let hit = Hit::Width(i);
            let on = s.width == i;
            layer.rects.push(UiRect {
                rect: sb,
                fill: if on { p.raised } else { [0.0; 4] },
                border: p.accent,
                radius: CONTROL_RADIUS,
                border_width: if on || cx.focus == Some(hit) {
                    2.0
                } else {
                    0.0
                },
            });
            // A dot as wide as the stroke, the honest preview of a width.
            let d = (w * 1.2).clamp(2.0, 14.0);
            layer.rects.push(UiRect {
                rect: rect(sb[0] + (24.0 - d) / 2.0, sb[1] + (24.0 - d) / 2.0, d, d),
                fill: p.text,
                border: [0.0; 4],
                radius: d / 2.0,
                border_width: 0.0,
            });
            cx.region(sb, hit, copy::WIDTH_NAMES[i], Role::Toggle { on }, true);
        }
        if fade < 1.0 {
            for r in &mut layer.rects[first_rect..] {
                r.fill = with_alpha(r.fill, fade);
                r.border = with_alpha(r.border, fade);
            }
            for t in &mut layer.texts[first_text..] {
                t.color = with_alpha(t.color, fade);
            }
        }
    }

    fn zoom_readout(cx: &mut Ctx, layer: &mut UiLayer, s: &Status, size: Vec2, right: f32) {
        let label = copy::zoom(s.zoom);
        let w = cx.measure.width(&label, SMALL, Font::Mono) + 16.0;
        let b = rect(size.x - right - PAD - w, size.y - PAD - 24.0, w, 24.0);
        let hit = Hit::ZoomToFit;
        let p = cx.p;
        layer.rects.push(UiRect {
            rect: b,
            fill: if cx.hover == Some(hit) {
                p.raised
            } else {
                p.panel
            },
            border: if cx.focus == Some(hit) {
                p.accent
            } else {
                p.border
            },
            radius: CONTROL_RADIUS,
            border_width: if cx.focus == Some(hit) { 2.0 } else { 1.0 },
        });
        cx.text(
            layer,
            &label,
            Vec2::new(b[0] + 8.0, b[1] + (24.0 - text_height(SMALL)) / 2.0),
            SMALL,
            Font::Mono,
            p.muted,
        );
        cx.region(
            b,
            hit,
            format!("{label}, {}", copy::ZOOM_TO_FIT),
            Role::Button,
            true,
        );
    }

    fn sidebar(&self, cx: &mut Ctx, layer: &mut UiLayer, s: &Status, top: f32, size: Vec2) {
        let p = cx.p;
        let b = rect(0.0, top, SIDE_W, size.y - top);
        layer.rects.push(UiRect {
            rect: b,
            fill: p.panel,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        layer.rects.push(UiRect {
            rect: rect(SIDE_W - 1.0, top, 1.0, size.y - top),
            fill: p.border,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        cx.region(b, Hit::Panel, copy::SKETCHES, Role::Group, false);
        let mut y = top + PAD - self.scroll;
        cx.text(
            layer,
            copy::PROJECTS,
            Vec2::new(PAD, y + 6.0),
            TITLE,
            Font::SansStrong,
            p.text,
        );
        let w = cx.measure.width(copy::NEW_PROJECT, BODY, Font::Sans) + 16.0;
        cx.button(
            layer,
            rect(SIDE_W - PAD - w + 4.0, y, w, ROW),
            copy::NEW_PROJECT,
            None,
            Hit::NewProject,
            false,
            Role::Button,
        );
        y += ROW + 12.0;
        for project in s.projects {
            let pid = project.id;
            self.field(
                cx,
                layer,
                Vec2::new(PAD - 4.0, y),
                SIDE_W - 2.0 * PAD - 80.0,
                &project.name,
                Field::ProjectName(pid),
                Font::SansStrong,
            );
            let w = cx.measure.width(copy::NEW_SKETCH, SMALL, Font::Sans) + 12.0;
            let nb = rect(SIDE_W - PAD - w + 4.0, y + 2.0, w, ROW - 4.0);
            let hit = Hit::NewSketch(pid);
            layer.rects.push(UiRect {
                rect: nb,
                fill: if cx.hover == Some(hit) {
                    p.raised
                } else {
                    [0.0; 4]
                },
                border: p.accent,
                radius: CONTROL_RADIUS,
                border_width: if cx.focus == Some(hit) { 2.0 } else { 0.0 },
            });
            cx.text(
                layer,
                copy::NEW_SKETCH,
                Vec2::new(nb[0] + 6.0, nb[1] + (nb[3] - text_height(SMALL)) / 2.0),
                SMALL,
                Font::Sans,
                p.muted,
            );
            cx.region(
                nb,
                hit,
                format!("{} in {}", copy::NEW_SKETCH, project.name),
                Role::Button,
                true,
            );
            y += ROW + 6.0;
            for sketch in &project.sketches {
                let row = rect(PAD - 6.0, y, SIDE_W - 2.0 * PAD + 12.0, THUMB.y + 12.0);
                let current = sketch.id == s.sketch;
                let hit = Hit::Sketch(sketch.id);
                layer.rects.push(UiRect {
                    rect: row,
                    fill: if current {
                        p.raised
                    } else if cx.hover == Some(hit) {
                        with_alpha(p.raised, 0.6)
                    } else {
                        [0.0; 4]
                    },
                    border: p.accent,
                    radius: CONTROL_RADIUS,
                    border_width: if cx.focus == Some(hit) { 2.0 } else { 0.0 },
                });
                let tb = rect(row[0] + 6.0, row[1] + 6.0, THUMB.x, THUMB.y);
                layer.rects.push(UiRect {
                    rect: tb,
                    fill: {
                        let [r, g, b] = p.canvas.background;
                        [r, g, b, 1.0]
                    },
                    border: p.border,
                    radius: 4.0,
                    border_width: 1.0,
                });
                layer.images.push(UiImage {
                    id: thumbnail_id(sketch.id),
                    rect: [tb[0] + 1.0, tb[1] + 1.0, tb[2] - 2.0, tb[3] - 2.0],
                    radius: 3.0,
                });
                let nx = tb[0] + THUMB.x + 10.0;
                let font = if current {
                    Font::SansStrong
                } else {
                    Font::Sans
                };
                layer.texts.push(UiText {
                    text: sketch.name.clone(),
                    at: [nx, row[1] + (row[3] - text_height(BODY)) / 2.0],
                    size: BODY,
                    font,
                    color: p.text,
                    width: Some(row[0] + row[2] - nx - 6.0),
                });
                cx.region(
                    row,
                    hit,
                    sketch.name.clone(),
                    Role::ListItem { selected: current },
                    true,
                );
                y += row[3] + 4.0;
            }
            y += 10.0;
        }
    }

    fn inspector(
        &self,
        cx: &mut Ctx,
        layer: &mut UiLayer,
        popup: &mut UiLayer,
        s: &Status,
        top: f32,
        size: Vec2,
    ) {
        let p = cx.p;
        let x0 = size.x - SIDE_W;
        let b = rect(x0, top, SIDE_W, size.y - top);
        layer.rects.push(UiRect {
            rect: b,
            fill: p.panel,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        layer.rects.push(UiRect {
            rect: rect(x0, top, 1.0, size.y - top),
            fill: p.border,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        cx.region(b, Hit::Panel, copy::INSPECTOR, Role::Group, false);
        let x = x0 + PAD;
        let mut y = top + PAD;
        let wrap = SIDE_W - 2.0 * PAD;
        let para = |layer: &mut UiLayer, y: &mut f32, text: &str, color: [f32; 4]| {
            layer.texts.push(UiText {
                text: text.into(),
                at: [x, *y],
                size: BODY,
                font: Font::Sans,
                color,
                width: Some(wrap),
            });
            *y += text_height(BODY) * 2.0 + 8.0;
        };
        let label = |cx: &mut Ctx, layer: &mut UiLayer, y: f32, text: &str| {
            cx.text(
                layer,
                text,
                Vec2::new(x, y + (ROW - text_height(SMALL)) / 2.0),
                SMALL,
                Font::Sans,
                p.muted,
            );
        };
        let num = |v: f64| format!("{:.0}", v);
        match &s.inspect {
            Inspect::Nothing => {
                para(layer, &mut y, copy::NOTHING_SELECTED, p.muted);
                para(layer, &mut y, &s.summary, p.muted);
                Self::style_section(cx, layer, x, y, wrap, &s.style);
            }
            Inspect::Elements {
                count,
                bounds,
                width,
            } => {
                cx.text(
                    layer,
                    &copy::selection_count(*count),
                    Vec2::new(x, y),
                    TITLE,
                    Font::SansStrong,
                    p.text,
                );
                y += 30.0;
                self.geometry(cx, layer, x, &mut y, *bounds, &label, &num);
                if let Some(w) = width {
                    label(cx, layer, y, copy::INSPECT_STROKE);
                    self.field(
                        cx,
                        layer,
                        Vec2::new(x + 96.0, y),
                        64.0,
                        &format!("{w}"),
                        Field::StrokeWidth,
                        Font::Mono,
                    );
                }
            }
            Inspect::Wire {
                shape_type,
                bounds,
                confidence,
                uncertain,
                label: text,
                pinned,
            } => {
                cx.text(
                    layer,
                    shape_type.display_name(),
                    Vec2::new(x, y),
                    TITLE,
                    Font::SansStrong,
                    p.text,
                );
                y += 30.0;
                label(cx, layer, y, copy::INSPECT_TYPE);
                let tb = rect(x + 96.0, y, wrap - 96.0, ROW);
                cx.button(
                    layer,
                    tb,
                    shape_type.display_name(),
                    None,
                    Hit::TypeMenu,
                    false,
                    Role::Button,
                );
                if self.type_menu {
                    let h = ShapeType::ALL.len() as f32 * 24.0 + 8.0;
                    let mb = rect(tb[0], (tb[1] + ROW + 4.0).min(size.y - h - PAD), tb[2], h);
                    cx.panel(popup, mb, CONTROL_RADIUS);
                    cx.region(mb, Hit::Panel, copy::INSPECT_TYPE, Role::Group, false);
                    for (i, t) in ShapeType::ALL.iter().enumerate() {
                        let ob = rect(
                            mb[0] + 4.0,
                            mb[1] + 4.0 + i as f32 * 24.0,
                            mb[2] - 8.0,
                            24.0,
                        );
                        cx.button(
                            popup,
                            ob,
                            t.display_name(),
                            None,
                            Hit::TypeOption(*t),
                            t == shape_type,
                            Role::ListItem {
                                selected: t == shape_type,
                            },
                        );
                    }
                }
                y += ROW + 6.0;
                label(cx, layer, y, copy::INSPECT_LABEL);
                self.field(
                    cx,
                    layer,
                    Vec2::new(x + 96.0, y),
                    wrap - 96.0,
                    text,
                    Field::Label,
                    Font::Sans,
                );
                y += ROW + 6.0;
                self.geometry(cx, layer, x, &mut y, *bounds, &label, &num);
                label(cx, layer, y, copy::INSPECT_CONFIDENCE);
                let c = format!("{:.0} %", confidence * 100.0);
                cx.text(
                    layer,
                    &c,
                    Vec2::new(x + 104.0, y + (ROW - text_height(SMALL)) / 2.0),
                    SMALL,
                    Font::Mono,
                    p.text,
                );
                y += ROW + 6.0;
                if *uncertain {
                    para(layer, &mut y, copy::INSPECT_UNCERTAIN, p.warning);
                }
                if *pinned {
                    cx.text_button(
                        layer,
                        Vec2::new(x - 8.0, y),
                        copy::RESET_WIREFRAME,
                        Hit::ResetWireframe,
                        false,
                    );
                }
            }
        }
    }

    /// The sketch's style: what was read off a screenshot (with its WCAG warnings) or what is applied,
    /// import, and the saved presets.
    fn style_section(
        cx: &mut Ctx,
        layer: &mut UiLayer,
        x: f32,
        mut y: f32,
        wrap: f32,
        v: &StyleView,
    ) {
        let p = cx.p;
        layer.rects.push(UiRect {
            rect: rect(x, y, wrap, 1.0),
            fill: p.border,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        y += 12.0;
        cx.text(
            layer,
            copy::STYLE,
            Vec2::new(x, y),
            TITLE,
            Font::SansStrong,
            p.text,
        );
        y += 28.0;
        let small = |cx: &mut Ctx, layer: &mut UiLayer, y: &mut f32, text: &str, color| {
            layer.texts.push(UiText {
                text: text.into(),
                at: [x, *y],
                size: SMALL,
                font: Font::Sans,
                color,
                width: Some(wrap),
            });
            let lines = (cx.measure.width(text, SMALL, Font::Sans) / wrap)
                .ceil()
                .max(1.0);
            *y += lines * (text_height(SMALL) + 2.0) + 6.0;
        };
        if v.reading {
            small(cx, layer, &mut y, copy::READING_STYLE, p.muted);
        }
        match v.pending.map(|(s, _)| s).or(v.applied) {
            Some(style) => {
                let colours = [style.background, style.surface, style.accent, style.text];
                for (role, c) in copy::STYLE_ROLES.into_iter().zip(colours) {
                    let row = rect(x, y, wrap, 22.0);
                    layer.rects.push(UiRect {
                        rect: rect(x, y + 2.0, 18.0, 18.0),
                        fill: theme::hex(c),
                        border: p.border,
                        radius: 4.0,
                        border_width: 1.0,
                    });
                    let ty = y + (22.0 - text_height(SMALL)) / 2.0;
                    cx.text(
                        layer,
                        role,
                        Vec2::new(x + 28.0, ty),
                        SMALL,
                        Font::Sans,
                        p.text,
                    );
                    cx.text(
                        layer,
                        &format!("#{c:06x}"),
                        Vec2::new(x + 120.0, ty),
                        SMALL,
                        Font::Mono,
                        p.muted,
                    );
                    cx.region(row, Hit::Panel, copy::swatch(role, c), Role::Label, false);
                    y += 24.0;
                }
                y += 4.0;
                let metrics = copy::style_metrics(style.radius, style.spacing, style.line_height);
                small(cx, layer, &mut y, &metrics, p.muted);
                if let Some((style, notes)) = v.pending {
                    for c in style.failing() {
                        small(
                            cx,
                            layer,
                            &mut y,
                            &copy::low_contrast(c.pair, c.ratio),
                            p.warning,
                        );
                    }
                    for n in notes {
                        small(cx, layer, &mut y, copy::style_note(*n), p.warning);
                    }
                }
                let buttons: [(&str, Hit, bool); 2] = if v.pending.is_some() {
                    [
                        (copy::APPLY_STYLE, Hit::ApplyStyle, true),
                        (copy::DISCARD_STYLE, Hit::DiscardStyle, false),
                    ]
                } else {
                    [
                        (copy::SAVE_PRESET, Hit::SavePreset, false),
                        (copy::REMOVE_STYLE, Hit::RemoveStyle, false),
                    ]
                };
                let mut bx = x - 8.0;
                for (label, hit, primary) in buttons {
                    bx += cx.text_button(layer, Vec2::new(bx, y), label, hit, primary) + 4.0;
                }
                y += ROW + 6.0;
            }
            None if !v.reading => small(cx, layer, &mut y, copy::STYLE_HINT, p.muted),
            None => {}
        }
        cx.text_button(
            layer,
            Vec2::new(x - 8.0, y),
            copy::IMPORT_STYLE,
            Hit::ImportStyle,
            false,
        );
        y += ROW + 12.0;
        if v.presets.is_empty() {
            return;
        }
        cx.text(
            layer,
            copy::PRESETS,
            Vec2::new(x, y),
            SMALL,
            Font::SansStrong,
            p.muted,
        );
        y += 22.0;
        for preset in v.presets {
            let applied = v.applied == Some(&preset.style) && v.pending.is_none();
            let del_w = cx.measure.width(copy::DELETE_PRESET, BODY, Font::Sans) + 16.0;
            cx.button(
                layer,
                rect(x - 8.0, y, wrap + 8.0 - del_w - 4.0, ROW),
                &preset.name,
                None,
                Hit::Preset(preset.id),
                applied,
                Role::ListItem { selected: applied },
            );
            cx.text_button(
                layer,
                Vec2::new(x + wrap - del_w, y),
                copy::DELETE_PRESET,
                Hit::DeletePreset(preset.id),
                false,
            );
            // One "Delete" per row on screen; a screen reader needs to hear which.
            if let Some(r) = cx.regions.last_mut() {
                r.label = copy::delete_preset(&preset.name);
            }
            y += ROW + 2.0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn geometry(
        &self,
        cx: &mut Ctx,
        layer: &mut UiLayer,
        x: f32,
        y: &mut f32,
        b: Rect,
        label: &dyn Fn(&mut Ctx, &mut UiLayer, f32, &str),
        num: &dyn Fn(f64) -> String,
    ) {
        for (title, a, fa, bv, fb) in [
            (copy::INSPECT_POSITION, b.x0, Field::X, b.y0, Field::Y),
            (
                copy::INSPECT_SIZE,
                b.width(),
                Field::W,
                b.height(),
                Field::H,
            ),
        ] {
            label(cx, layer, *y, title);
            self.field(
                cx,
                layer,
                Vec2::new(x + 96.0, *y),
                72.0,
                &num(a),
                fa,
                Font::Mono,
            );
            self.field(
                cx,
                layer,
                Vec2::new(x + 172.0, *y),
                72.0,
                &num(bv),
                fb,
                Font::Mono,
            );
            *y += ROW + 6.0;
        }
    }

    fn toasts_(&self, cx: &mut Ctx, layer: &mut UiLayer, size: Vec2) {
        let p = cx.p;
        let mut y = size.y - PAD - 24.0 - 12.0;
        for t in self.toasts.iter().rev() {
            let action = t.action.map(|a| match a {
                ToastAction::Retry => copy::RETRY,
                ToastAction::Review => copy::REVIEW,
                ToastAction::OpenFolder => copy::OPEN_FOLDER,
            });
            let mw = cx
                .measure
                .width(&t.message, BODY, Font::Sans)
                .min(size.x * 0.6);
            let aw = action.map_or(0.0, |a| cx.measure.width(a, BODY, Font::Sans) + 16.0 + 8.0);
            let progress = t.kind == ToastKind::Progress;
            let dw = if progress {
                0.0
            } else {
                cx.measure.width(copy::DISMISS, BODY, Font::Sans) + 16.0
            };
            let w = PAD + mw + 16.0 + aw + dw + 8.0;
            let h = 40.0;
            y -= h;
            let b = rect((size.x - w) / 2.0, y, w, h);
            layer.rects.push(UiRect {
                rect: b,
                fill: p.panel,
                border: if t.kind == ToastKind::Error {
                    p.error
                } else {
                    p.border
                },
                radius: PANEL_RADIUS,
                border_width: 1.0,
            });
            cx.region(b, Hit::Panel, t.message.clone(), Role::Label, false);
            layer.texts.push(UiText {
                text: t.message.clone(),
                at: [b[0] + PAD, b[1] + (h - text_height(BODY)) / 2.0],
                size: BODY,
                font: Font::Sans,
                color: if t.kind == ToastKind::Error {
                    p.error
                } else {
                    p.text
                },
                width: Some(mw + 4.0),
            });
            let mut bx = b[0] + PAD + mw + 16.0;
            if let (Some(a), Some(kind)) = (action, t.action) {
                let w = cx.text_button(
                    layer,
                    Vec2::new(bx, b[1] + 6.0),
                    a,
                    Hit::Toast(t.id, kind),
                    false,
                );
                bx += w + 8.0;
            }
            if !progress {
                cx.text_button(
                    layer,
                    Vec2::new(bx, b[1] + 6.0),
                    copy::DISMISS,
                    Hit::DismissToast(t.id),
                    false,
                );
            }
            y -= 8.0;
        }
    }

    fn dialog_frame(
        cx: &mut Ctx,
        layer: &mut UiLayer,
        size: Vec2,
        w: f32,
        h: f32,
        title: &str,
        closable: bool,
    ) -> Bounds {
        let p = cx.p;
        layer.rects.push(UiRect {
            rect: rect(0.0, 0.0, size.x, size.y),
            fill: p.scrim,
            border: [0.0; 4],
            radius: 0.0,
            border_width: 0.0,
        });
        cx.modal = Some(cx.regions.len());
        // Clicking the scrim closes a closable dialog; otherwise it only keeps clicks off the canvas.
        let (scrim, label) = if closable {
            (Hit::CloseDialog, copy::CLOSE)
        } else {
            (Hit::Panel, "")
        };
        cx.region(
            rect(0.0, 0.0, size.x, size.y),
            scrim,
            label,
            Role::Button,
            false,
        );
        let b = rect(
            ((size.x - w) / 2.0).max(PAD),
            ((size.y - h) / 2.0).max(PAD),
            w.min(size.x - 2.0 * PAD),
            h.min(size.y - 2.0 * PAD),
        );
        cx.panel(layer, b, PANEL_RADIUS);
        cx.region(b, Hit::Panel, title, Role::Group, false);
        cx.text(
            layer,
            title,
            Vec2::new(b[0] + 20.0, b[1] + 18.0),
            TITLE,
            Font::SansStrong,
            p.text,
        );
        if !closable {
            return b;
        }
        // Room for the label and its "Esc" hint.
        let cw = cx.measure.width(copy::CLOSE, BODY, Font::Sans)
            + cx.measure.width("Esc", SMALL, Font::Mono)
            + 28.0;
        cx.button(
            layer,
            rect(b[0] + b[2] - cw - 12.0, b[1] + 12.0, cw, ROW),
            copy::CLOSE,
            Some("Esc"),
            Hit::CloseDialog,
            false,
            Role::Button,
        );
        b
    }

    /// The store can't be opened: what happened and the three ways on. It can't be closed.
    fn store_dialog(cx: &mut Ctx, layer: &mut UiLayer, reason: &str, size: Vec2) {
        let p = cx.p;
        let b = Self::dialog_frame(cx, layer, size, 520.0, 196.0, copy::STORE_TITLE, false);
        let message = copy::store_unavailable(reason);
        cx.region(b, Hit::Panel, message.clone(), Role::Label, false);
        layer.texts.push(UiText {
            text: message,
            at: [b[0] + 20.0, b[1] + 52.0],
            size: BODY,
            font: Font::Sans,
            color: p.text,
            width: Some(b[2] - 40.0),
        });
        // Right to left, the safe default (Retry) last and filled.
        let mut x = b[0] + b[2] - 20.0;
        let y = b[1] + b[3] - 20.0 - ROW;
        for (label, hit, primary) in [
            (copy::RETRY, Hit::Retry, true),
            (copy::CHOOSE_LOCATION, Hit::ChooseLocation, false),
            (copy::QUIT, Hit::Quit, false),
        ] {
            let w = cx.measure.width(label, BODY, Font::Sans) + 24.0;
            x -= w;
            cx.button(
                layer,
                rect(x, y, w, ROW),
                label,
                None,
                hit,
                primary,
                Role::Button,
            );
            x -= 8.0;
        }
    }

    fn shortcuts(cx: &mut Ctx, layer: &mut UiLayer, size: Vec2) {
        let p = cx.p;
        let b = Self::dialog_frame(cx, layer, size, 760.0, 470.0, copy::SHORTCUTS, true);
        let col_w = (b[2] - 40.0) / 2.0;
        let (mut cx_x, mut y0) = (b[0] + 20.0, b[1] + 56.0);
        let mut y = y0;
        for (i, (group, rows)) in copy::SHORTCUT_GROUPS.iter().enumerate() {
            if i == 2 {
                cx_x += col_w + 20.0;
                y0 = b[1] + 56.0;
                y = y0;
            }
            cx.text(
                layer,
                group,
                Vec2::new(cx_x, y),
                BODY,
                Font::SansStrong,
                p.text,
            );
            y += 24.0;
            for (what, keys) in rows.iter() {
                cx.text(layer, what, Vec2::new(cx_x, y), BODY, Font::Sans, p.muted);
                let kw = cx.measure.width(keys, SMALL, Font::Mono);
                cx.text(
                    layer,
                    keys,
                    Vec2::new(cx_x + col_w - 40.0 - kw, y + 1.0),
                    SMALL,
                    Font::Mono,
                    p.text,
                );
                cx.region(
                    rect(cx_x, y, col_w - 40.0, 20.0),
                    Hit::Panel,
                    format!("{what}: {keys}"),
                    Role::Label,
                    false,
                );
                y += 20.0;
            }
            y += 12.0;
        }
    }

    fn preferences(cx: &mut Ctx, layer: &mut UiLayer, s: &Status, size: Vec2) {
        let p = cx.p;
        let b = Self::dialog_frame(cx, layer, size, 520.0, 250.0, copy::PREFERENCES, true);
        let x = b[0] + 20.0;
        let mut y = b[1] + 60.0;
        let set = s.settings;
        let rows: [(&str, Vec<PrefOption>); 4] = [
            (
                copy::THEME,
                [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light]
                    .iter()
                    .zip(copy::THEMES)
                    .map(|(t, l)| (l, Pref::Theme(*t), set.theme == *t))
                    .collect(),
            ),
            (
                copy::GRID,
                vec![
                    (copy::ON_OFF[0], Pref::Grid(true), set.grid),
                    (copy::ON_OFF[1], Pref::Grid(false), !set.grid),
                ],
            ),
            (
                copy::SMOOTHING,
                vec![
                    (copy::ON_OFF[0], Pref::Smoothing(true), set.smoothing),
                    (copy::ON_OFF[1], Pref::Smoothing(false), !set.smoothing),
                ],
            ),
            (
                copy::PRESSURE,
                [
                    PressureCurve::Soft,
                    PressureCurve::Linear,
                    PressureCurve::Firm,
                ]
                .iter()
                .zip(copy::PRESSURES)
                .map(|(c, l)| (l, Pref::Pressure(*c), set.pressure == *c))
                .collect(),
            ),
        ];
        for (title, options) in rows {
            cx.text(
                layer,
                title,
                Vec2::new(x, y + (ROW - text_height(BODY)) / 2.0),
                BODY,
                Font::Sans,
                p.text,
            );
            let mut ox = x + 150.0;
            for (label, pref, on) in options {
                let w = cx.measure.width(label, BODY, Font::Sans) + 16.0;
                cx.button(
                    layer,
                    rect(ox, y, w, ROW),
                    label,
                    None,
                    Hit::Pref(pref),
                    on,
                    Role::Toggle { on },
                );
                // Name the setting too: "On" alone means nothing to a screen reader.
                let last = cx.regions.len() - 1;
                cx.regions[last].label = format!("{title}: {label}");
                ox += w + 4.0;
            }
            y += ROW + 12.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nc_store::SketchInfo;

    /// Every character 7 px wide: enough to lay out.
    struct Fixed;

    impl Measure for Fixed {
        fn width(&mut self, text: &str, size: f32, _: Font) -> f32 {
            text.chars().count() as f32 * size * 0.55
        }
    }

    fn status(projects: &[ProjectInfo], sketch: Uuid) -> Status<'_> {
        Status {
            dark: true,
            tool: Tool::Pen,
            ink: 0,
            width: 1,
            zoom: 1.0,
            wire_level: 0.0,
            split: false,
            projects,
            sketch,
            save: SaveStatus::Saved,
            quality: None,
            inspect: Inspect::Nothing,
            empty: true,
            drawing: false,
            summary: copy::canvas_summary(0, 0, 0),
            settings: Settings::default(),
            loading: false,
            store_error: None,
            style: StyleView::default(),
        }
    }

    fn projects() -> (Vec<ProjectInfo>, Uuid) {
        let sketch = Uuid::from_u128(7);
        let p = ProjectInfo {
            id: Uuid::from_u128(1),
            name: "My Project".into(),
            sketches: vec![
                SketchInfo {
                    id: sketch,
                    name: "Login".into(),
                },
                SketchInfo {
                    id: Uuid::from_u128(8),
                    name: "Settings".into(),
                },
            ],
        };
        (vec![p], sketch)
    }

    fn built(shell: &mut Shell, s: &Status) {
        shell.build(s, Vec2::new(1280.0, 800.0), &mut Fixed);
    }

    /// With nothing selected, the inspector shows the style: a screenshot's read with its WCAG
    /// warnings and Apply, or the applied style with its presets.
    #[test]
    fn inspector_shows_style_warnings_and_presets() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        shell.inspector = true;
        let pale = Style {
            background: 0xffffff,
            surface: 0xf4f5f7,
            accent: 0x9fc4ff,
            text: 0xaaaaaa,
            radius: 8.0,
            spacing: 8.0,
            line_height: None,
        };
        let notes = [Note::NoBoxes];
        let mut s = status(&projects, sketch);
        s.style.pending = Some((&pale, &notes));
        built(&mut shell, &s);
        let texts: Vec<&str> = shell
            .scene()
            .base
            .texts
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        let warning = copy::low_contrast(
            nc_core::style::Pair::TextOnBackground,
            pale.check(nc_core::style::Pair::TextOnBackground).ratio,
        );
        assert!(texts.contains(&warning.as_str()), "{texts:?}");
        assert!(texts.contains(&copy::style_note(Note::NoBoxes)));
        assert!(texts.contains(&"#9fc4ff"));
        find(&shell, Hit::ApplyStyle);
        find(&shell, Hit::DiscardStyle);
        assert!(shell.regions().iter().any(|r| r.label == "Accent #9fc4ff"));

        let presets = [Preset {
            id: Uuid::from_u128(3),
            name: "Files".into(),
            style: pale,
        }];
        let mut s = status(&projects, sketch);
        s.style.applied = Some(&pale);
        s.style.presets = &presets;
        built(&mut shell, &s);
        find(&shell, Hit::RemoveStyle);
        find(&shell, Hit::SavePreset);
        assert!(shell.regions().iter().all(|r| r.hit != Hit::ApplyStyle));
        let row = shell
            .regions()
            .iter()
            .find(|r| r.hit == Hit::Preset(presets[0].id))
            .unwrap();
        assert_eq!(
            row.role,
            Role::ListItem { selected: true },
            "the applied preset is marked"
        );
        let del = shell
            .regions()
            .iter()
            .find(|r| r.hit == Hit::DeletePreset(presets[0].id))
            .unwrap();
        assert_eq!(del.label, copy::delete_preset("Files"));
        let texts: Vec<&str> = shell
            .scene()
            .base
            .texts
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        assert!(
            !texts.contains(&warning.as_str()),
            "warnings belong to the review before applying"
        );
    }

    fn find(shell: &Shell, hit: Hit) -> Bounds {
        shell
            .regions()
            .iter()
            .find(|r| r.hit == hit)
            .unwrap_or_else(|| panic!("{hit:?} not laid out"))
            .bounds
    }

    fn centre(b: Bounds) -> Vec2 {
        Vec2::new(b[0] + b[2] / 2.0, b[1] + b[3] / 2.0)
    }

    #[test]
    fn every_tool_and_bar_control_is_clickable_where_it_is_drawn() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        built(&mut shell, &status(&projects, sketch));
        for (tool, _, _) in copy::TOOLS {
            let b = find(&shell, Hit::Tool(tool));
            assert_eq!(shell.hit(centre(b)), Some(Hit::Tool(tool)));
        }
        for hit in [
            Hit::Sidebar,
            Hit::Inspector,
            Hit::Export,
            Hit::Shortcuts,
            Hit::ZoomToFit,
            Hit::Mode(2),
        ] {
            assert_eq!(shell.hit(centre(find(&shell, hit))), Some(hit));
        }
        // Empty canvas: the hint shows, and the middle of the window is canvas, not chrome.
        assert!(
            shell
                .scene()
                .base
                .texts
                .iter()
                .any(|t| t.text == copy::EMPTY_HINT)
        );
        assert!(!shell.covers(Vec2::new(640.0, 500.0)));
    }

    #[test]
    fn sidebar_lists_sketches_and_marks_the_open_one() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        shell.sidebar = true;
        built(&mut shell, &status(&projects, sketch));
        let open = shell
            .regions()
            .iter()
            .find(|r| r.hit == Hit::Sketch(sketch))
            .unwrap();
        assert_eq!(open.role, Role::ListItem { selected: true });
        assert!(
            shell
                .regions()
                .iter()
                .any(|r| r.hit == Hit::Sketch(Uuid::from_u128(8)))
        );
        assert!(
            shell
                .scene()
                .base
                .images
                .iter()
                .any(|i| i.id == thumbnail_id(sketch))
        );
        // The palette moves right of the sidebar.
        assert!(find(&shell, Hit::Tool(Tool::Pen))[0] > SIDE_W);
    }

    #[test]
    fn tab_walks_every_control_then_returns_to_the_canvas() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        built(&mut shell, &status(&projects, sketch));
        let focusable = shell.regions().iter().filter(|r| r.focusable).count();
        assert!(focusable > 15);
        let mut seen = Vec::new();
        for _ in 0..focusable {
            shell.focus_next(false);
            seen.push(shell.focus.unwrap());
        }
        shell.focus_next(false);
        assert_eq!(
            shell.focus, None,
            "past the last control: back to the canvas"
        );
        seen.dedup();
        assert_eq!(seen.len(), focusable, "each control once");
        shell.focus_next(true);
        assert_eq!(
            shell.focus,
            Some(*seen.last().unwrap()),
            "Shift+Tab from the canvas"
        );
    }

    #[test]
    fn every_region_has_a_label_and_focus_draws_a_ring() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        shell.sidebar = true;
        shell.inspector = true;
        shell.dialog = Dialog::Preferences;
        built(&mut shell, &status(&projects, sketch));
        for r in shell.regions().iter().filter(|r| r.focusable) {
            assert!(!r.label.is_empty(), "{:?} has no accessible label", r.hit);
        }
        shell.focus = Some(Hit::Export);
        let before = shell.scene().base.rects.len();
        built(&mut shell, &status(&projects, sketch));
        assert_eq!(shell.scene().base.rects.len(), before);
        let b = find(&shell, Hit::Export);
        let ring = shell
            .scene()
            .base
            .rects
            .iter()
            .find(|r| r.rect == b)
            .unwrap();
        assert_eq!(ring.border_width, 2.0, "visible focus (UX principle 9)");
    }

    #[test]
    fn text_fields_edit_and_commit() {
        let mut shell = Shell::new();
        let field = Field::SketchName(Uuid::from_u128(7));
        shell.begin_edit(field, "Login".into());
        for k in [
            EditKey::Backspace,
            EditKey::Char('n'),
            EditKey::Home,
            EditKey::Char('A'),
        ] {
            assert_eq!(shell.edit_key(k), None);
        }
        assert_eq!(shell.edit.as_ref().unwrap().text, "ALogin");
        assert_eq!(
            shell.edit_key(EditKey::Enter),
            Some((field, "ALogin".into()))
        );
        assert!(shell.edit.is_none());
        shell.begin_edit(field, "é".into());
        shell.edit_key(EditKey::Backspace);
        assert_eq!(
            shell.edit.as_ref().unwrap().text,
            "",
            "deletes whole characters"
        );
        shell.edit_key(EditKey::Escape);
        assert!(shell.edit.is_none(), "Escape cancels");
    }

    #[test]
    fn toasts_expire_unless_they_are_errors() {
        let mut shell = Shell::new();
        shell.toast(
            ToastKind::Info,
            "Exported",
            Some(ToastAction::OpenFolder),
            0.0,
        );
        let err = shell.toast(ToastKind::Error, "Export failed", None, 0.0);
        assert_eq!(shell.tick(1.0), Some(TOAST_SECS));
        assert_eq!(shell.tick(TOAST_SECS + 0.1), None);
        assert_eq!(shell.toasts.len(), 1, "the error stays");
        shell.dismiss(err);
        assert!(shell.toasts.is_empty());
    }

    #[test]
    fn save_failure_shows_a_banner_with_retry() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        let mut s = status(&projects, sketch);
        s.save = SaveStatus::Failed("disk full".into());
        built(&mut shell, &s);
        assert!(shell.regions().iter().any(|r| r.hit == Hit::Retry));
        assert!(
            shell
                .scene()
                .base
                .texts
                .iter()
                .any(|t| t.text == copy::not_saved("disk full"))
        );
    }

    #[test]
    fn the_palette_steps_back_while_drawing() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        let mut s = status(&projects, sketch);
        s.drawing = true;
        built(&mut shell, &s);
        let pen = shell
            .scene()
            .base
            .texts
            .iter()
            .find(|t| t.text == "Pen")
            .unwrap();
        assert!(pen.color[3] < 0.5);
    }

    #[test]
    fn pressure_curves_keep_the_ends() {
        for c in [
            PressureCurve::Soft,
            PressureCurve::Linear,
            PressureCurve::Firm,
        ] {
            assert_eq!(c.apply(0.0), 0.0);
            assert_eq!(c.apply(1.0), 1.0);
        }
        assert!(PressureCurve::Soft.apply(0.5) > 0.5 && PressureCurve::Firm.apply(0.5) < 0.5);
    }

    #[test]
    fn hovering_a_bar_control_shows_its_shortcut() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        built(&mut shell, &status(&projects, sketch));
        shell.hover = Some(Hit::Sidebar);
        built(&mut shell, &status(&projects, sketch));
        assert!(
            shell
                .scene()
                .popup
                .texts
                .iter()
                .any(|t| t.text == copy::TIP_SKETCHES)
        );
        assert!(copy::TIP_SKETCHES.ends_with("Ctrl+B"));
    }

    #[test]
    fn progress_toasts_stay_and_cannot_be_dismissed() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        shell.toast(ToastKind::Progress, copy::EXPORTING, None, 0.0);
        assert_eq!(shell.tick(100.0), None);
        assert_eq!(shell.toasts.len(), 1);
        built(&mut shell, &status(&projects, sketch));
        assert!(
            !shell
                .regions()
                .iter()
                .any(|r| matches!(r.hit, Hit::DismissToast(_)))
        );
    }

    /// Mac bug #4: an unopenable store blocks everything behind a dialog that offers a way on and
    /// can't be dismissed.
    #[test]
    fn store_dialog_blocks_and_offers_retry_location_quit() {
        let (projects, sketch) = projects();
        let mut shell = Shell::new();
        let mut s = status(&projects, sketch);
        s.store_error = Some("permission denied");
        built(&mut shell, &s);
        let hits: Vec<Hit> = shell.regions().iter().map(|r| r.hit).collect();
        for h in [Hit::Retry, Hit::ChooseLocation, Hit::Quit] {
            assert!(hits.contains(&h), "{h:?}");
        }
        assert!(!hits.contains(&Hit::CloseDialog));
        // The scrim is topmost everywhere outside the dialog: nothing reaches the canvas or the bar.
        assert_eq!(shell.hit(Vec2::new(5.0, 5.0)), Some(Hit::Panel));
        assert_eq!(shell.hit(Vec2::new(5.0, 700.0)), Some(Hit::Panel));
        // Tab stays on the dialog's three buttons, wrapping.
        let mut seen = Vec::new();
        for _ in 0..4 {
            shell.focus_next(false);
            seen.push(shell.focus.unwrap());
        }
        assert_eq!(seen[3], seen[0]);
        assert!(
            seen.iter()
                .all(|h| matches!(h, Hit::Retry | Hit::ChooseLocation | Hit::Quit))
        );
    }
}
