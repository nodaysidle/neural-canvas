//! A second event queue on winit's own `wl_display`, for what winit 0.30 does not expose:
//! `wp_presentation` feedback (when a frame reached the screen, and the refresh period) and
//! `zwp_tablet_v2` tool input (pressure, eraser end). winit reads the socket; we only dispatch what it
//! already queued for us, so nothing here blocks.
//!
//! Drop this before winit closes the display: destroying proxies on a dead display segfaults.

use std::collections::HashMap;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_pointer::{self, WlPointer},
    wl_registry,
    wl_seat::{self, WlSeat},
    wl_surface::WlSurface,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child,
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
    zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
    zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
    zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
    zwp_tablet_v2::{self, ZwpTabletV2},
};
use winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};

/// CLOCK_MONOTONIC now, in ns: the clock `wp_presentation` reports on Hyprland (clock id 1).
pub fn mono_ns() -> u64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

/// What the compositor said about one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Presented {
    /// Scanout time (presentation clock, ns), the output's refresh period (ns, 0 if unknown) and flags.
    Shown {
        at_ns: u64,
        refresh_ns: u64,
        vsync: bool,
        zero_copy: bool,
    },
    Discarded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Pen,
    Eraser,
    Other,
}

/// Everything one tablet tool reported in one protocol frame, on our window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolFrame {
    pub kind: ToolKind,
    pub entered: bool,
    pub left: bool,
    pub down: bool,
    pub up: bool,
    pub moved: bool,
    /// Surface-local logical px.
    pub position: (f64, f64),
    /// 0..=1 when the tool reports pressure.
    pub pressure: Option<f32>,
    /// Compositor timestamp, ms.
    pub time_ms: u32,
}

#[derive(Debug)]
struct ToolState {
    frame: ToolFrame,
    has_pressure: bool,
    over_us: bool,
}

#[derive(Default)]
pub struct State {
    pub clock_id: Option<u32>,
    /// Names of tablets the compositor announced.
    pub tablets: Vec<String>,
    pub pads: usize,
    surface: Option<ObjectId>,
    presented: Vec<(u64, Presented)>,
    tool_frames: Vec<ToolFrame>,
    tools: HashMap<ObjectId, ToolState>,
    want_pointer: bool,
    pointer: Option<WlPointer>,
    pointer_times_ms: Vec<u32>,
}

pub struct SideChannel {
    queue: EventQueue<State>,
    pub state: State,
    surface: WlSurface,
    presentation: Option<WpPresentation>,
    tablet_seat: Option<ZwpTabletSeatV2>,
}

impl SideChannel {
    /// `None` when the window is not on Wayland. `pointer_times` also binds a `wl_pointer` so callers
    /// can read the compositor's motion timestamps (for input-latency measurement).
    pub fn new(
        window: &(impl HasWindowHandle + HasDisplayHandle + ?Sized),
        pointer_times: bool,
    ) -> Option<Self> {
        let RawDisplayHandle::Wayland(d) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(w) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit owns the display and surface; the caller drops us before winit closes them.
        let conn = Connection::from_backend(unsafe {
            Backend::from_foreign_display(d.display.as_ptr().cast())
        });
        let surface_id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), w.surface.as_ptr().cast()) }
                .ok()?;
        let surface = WlSurface::from_id(&conn, surface_id.clone()).ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let presentation = globals.bind::<WpPresentation, _, _>(&qh, 1..=2, ()).ok();
        let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=5, ()).ok();
        let tablet_seat = match (
            globals.bind::<ZwpTabletManagerV2, _, _>(&qh, 1..=1, ()),
            &seat,
        ) {
            (Ok(manager), Some(seat)) => Some(manager.get_tablet_seat(seat, &qh, ())),
            _ => None,
        };
        let mut state = State {
            surface: Some(surface_id),
            want_pointer: pointer_times,
            ..State::default()
        };
        queue.roundtrip(&mut state).ok()?;
        Some(Self {
            queue,
            state,
            surface,
            presentation,
            tablet_seat,
        })
    }

    pub fn has_presentation(&self) -> bool {
        self.presentation.is_some()
    }

    pub fn has_tablet_manager(&self) -> bool {
        self.tablet_seat.is_some()
    }

    /// Ask for feedback on the next commit of the window surface. Call right before presenting.
    pub fn request_feedback(&self, frame: u64) {
        if let Some(p) = &self.presentation {
            p.feedback(&self.surface, &self.queue.handle(), frame);
        }
    }

    /// Handles events winit has already read from the socket. Never reads the socket itself: that would
    /// consume data winit's event loop is waiting on, leaving winit's own events (pointer, keyboard)
    /// queued but undelivered until something else wakes it. Never blocks.
    pub fn dispatch(&mut self) {
        let _ = self.queue.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
    }

    /// Blocks until the compositor has answered everything sent so far.
    pub fn roundtrip(&mut self) {
        let _ = self.queue.roundtrip(&mut self.state);
    }

    /// Hands over presentation feedback received since the last call (frame id, result).
    pub fn drain_presented(&mut self, mut f: impl FnMut(u64, Presented)) {
        self.state.presented.drain(..).for_each(|(id, p)| f(id, p));
    }

    /// Hands over tablet tool frames received since the last call, in order.
    pub fn drain_tool_frames(&mut self, mut f: impl FnMut(ToolFrame)) {
        self.state.tool_frames.drain(..).for_each(&mut f);
    }

    /// Hands over `wl_pointer.motion` timestamps (ms) received since the last call.
    pub fn drain_pointer_times(&mut self, mut f: impl FnMut(u32)) {
        self.state.pointer_times_ms.drain(..).for_each(&mut f);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock_id = Some(clk_id);
        }
    }
}

impl Dispatch<WpPresentationFeedback, u64> for State {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        frame: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_presentation_feedback::{Event, Kind};
        let p = match event {
            Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                flags,
                ..
            } => {
                let secs = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                let flags = match flags {
                    WEnum::Value(f) => f,
                    WEnum::Unknown(bits) => Kind::from_bits_truncate(bits),
                };
                Presented::Shown {
                    at_ns: secs * 1_000_000_000 + tv_nsec as u64,
                    refresh_ns: refresh as u64,
                    vsync: flags.contains(Kind::Vsync),
                    zero_copy: flags.contains(Kind::ZeroCopy),
                }
            }
            Event::Discarded => Presented::Discarded,
            _ => return,
        };
        state.presented.push((*frame, p));
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && state.want_pointer
            && state.pointer.is_none()
            && caps.contains(wl_seat::Capability::Pointer)
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Motion { time, .. } = event {
            state.pointer_times_ms.push(time);
        }
    }
}

impl Dispatch<ZwpTabletManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpTabletManagerV2,
        _: <ZwpTabletManagerV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletSeatV2, ()> for State {
    event_created_child!(State, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);

    fn event(
        state: &mut Self,
        _: &ZwpTabletSeatV2,
        event: zwp_tablet_seat_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_tablet_seat_v2::Event::PadAdded { .. } = event {
            state.pads += 1;
        }
    }
}

impl Dispatch<ZwpTabletV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpTabletV2,
        event: zwp_tablet_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_tablet_v2::Event::Name { name } = event {
            state.tablets.push(name);
        }
    }
}

impl Dispatch<ZwpTabletToolV2, ()> for State {
    fn event(
        state: &mut Self,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_tablet_tool_v2::{Capability, Event, Type};
        let our_surface = state.surface.clone();
        let t = state.tools.entry(tool.id()).or_insert_with(|| ToolState {
            frame: ToolFrame {
                kind: ToolKind::Other,
                entered: false,
                left: false,
                down: false,
                up: false,
                moved: false,
                position: (0.0, 0.0),
                pressure: None,
                time_ms: 0,
            },
            has_pressure: false,
            over_us: false,
        });
        match event {
            Event::Type { tool_type } => {
                t.frame.kind = match tool_type {
                    WEnum::Value(Type::Eraser) => ToolKind::Eraser,
                    WEnum::Value(Type::Pen | Type::Pencil | Type::Brush | Type::Airbrush) => {
                        ToolKind::Pen
                    }
                    _ => ToolKind::Other,
                }
            }
            Event::Capability {
                capability: WEnum::Value(Capability::Pressure),
            } => t.has_pressure = true,
            Event::ProximityIn { surface, .. } => {
                t.over_us = Some(surface.id()) == our_surface;
                t.frame.entered = t.over_us;
            }
            Event::ProximityOut => t.frame.left = t.over_us,
            Event::Down { .. } => t.frame.down = true,
            Event::Up => t.frame.up = true,
            Event::Motion { x, y } => {
                t.frame.position = (x, y);
                t.frame.moved = true;
            }
            Event::Pressure { pressure } => {
                t.frame.pressure = Some(pressure.min(65_535) as f32 / 65_535.0);
            }
            Event::Frame { time } => {
                if t.over_us || t.frame.left {
                    let mut frame = t.frame;
                    frame.time_ms = time;
                    if !t.has_pressure {
                        frame.pressure = None;
                    }
                    state.tool_frames.push(frame);
                }
                if t.frame.left {
                    t.over_us = false;
                }
                let f = &mut t.frame;
                (f.entered, f.left, f.down, f.up, f.moved) = (false, false, false, false, false);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpTabletPadV2, ()> for State {
    event_created_child!(State, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);

    fn event(
        _: &mut Self,
        _: &ZwpTabletPadV2,
        _: zwp_tablet_pad_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadGroupV2, ()> for State {
    event_created_child!(State, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
    ]);

    fn event(
        _: &mut Self,
        _: &ZwpTabletPadGroupV2,
        _: zwp_tablet_pad_group_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(State: ignore ZwpTabletPadRingV2);
wayland_client::delegate_noop!(State: ignore ZwpTabletPadStripV2);
