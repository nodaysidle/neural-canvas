//! A second event queue on winit's own `wl_display`: `wp_presentation` feedback (when a frame actually
//! reached the screen, or was discarded) and a `zwp_tablet_v2` probe (winit 0.30 has no tablet support).
//! winit reads the socket; we only dispatch what it already queued for us.

use crate::stats::{Summary, over};
use std::collections::HashMap;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat, wl_surface::WlSurface};
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
#[derive(Clone, Copy, Debug)]
pub enum Presented {
    /// CLOCK_MONOTONIC ns at scanout, and the `vsync` / `zero_copy` flags.
    Shown {
        at_ns: u64,
        vsync: bool,
        zero_copy: bool,
    },
    Discarded,
}

#[derive(Default, Debug)]
pub struct TabletLog {
    pub tablets: Vec<String>,
    pub tools: Vec<String>,
    pub pads: usize,
    /// Tool frames that carried a motion event.
    pub motion_frames: u64,
    pub pressure_min: Option<u16>,
    pub pressure_max: Option<u16>,
    pub tilt_seen: bool,
    pub eraser_seen: bool,
}

#[derive(Default)]
pub struct State {
    pub clock_id: Option<u32>,
    pub presented: HashMap<u64, Presented>,
    pub tablet: TabletLog,
    pending_motion: bool,
}

pub struct SideChannel {
    queue: EventQueue<State>,
    pub state: State,
    surface: WlSurface,
    presentation: Option<WpPresentation>,
    tablet_seat: Option<ZwpTabletSeatV2>,
}

impl SideChannel {
    /// `None` when the window is not on Wayland.
    pub fn new(window: &(impl HasWindowHandle + HasDisplayHandle + ?Sized)) -> Option<Self> {
        let RawDisplayHandle::Wayland(d) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(w) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit owns the display and surface and outlives this harness's use of them.
        let conn = Connection::from_backend(unsafe {
            Backend::from_foreign_display(d.display.as_ptr().cast())
        });
        let surface_id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), w.surface.as_ptr().cast()) }
                .ok()?;
        let surface = WlSurface::from_id(&conn, surface_id).ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let presentation = globals.bind::<WpPresentation, _, _>(&qh, 1..=2, ()).ok();
        let tablet_seat = match (
            globals.bind::<ZwpTabletManagerV2, _, _>(&qh, 1..=1, ()),
            globals.bind::<WlSeat, _, _>(&qh, 1..=1, ()),
        ) {
            (Ok(manager), Ok(seat)) => Some(manager.get_tablet_seat(&seat, &qh, ())),
            _ => None,
        };
        let mut state = State::default();
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

    /// Handles events winit has already read from the socket. Never blocks.
    pub fn dispatch(&mut self) {
        let _ = self.queue.flush();
        if let Some(guard) = self.queue.prepare_read() {
            let _ = guard.read_without_dispatch();
        }
        let _ = self.queue.dispatch_pending(&mut self.state);
    }

    /// Prints pacing and latency for frames `0..starts.len()`; `starts[i]` is frame i's start in
    /// CLOCK_MONOTONIC ns (the moment its input state was sampled).
    pub fn print_scanout(&self, starts: &[u64]) {
        println!(
            "wp_presentation clock id: {:?} (1 = CLOCK_MONOTONIC)",
            self.state.clock_id
        );
        let (mut shown, mut latency, mut discarded, mut missing, mut zero_copy, mut vsync) =
            (vec![], vec![], 0, 0, 0, 0);
        for (i, start) in starts.iter().enumerate() {
            match self.state.presented.get(&(i as u64)) {
                Some(Presented::Shown {
                    at_ns,
                    vsync: v,
                    zero_copy: z,
                }) => {
                    shown.push(*at_ns);
                    latency.push(at_ns.saturating_sub(*start) as f64 / 1e6);
                    zero_copy += *z as usize;
                    vsync += *v as usize;
                }
                Some(Presented::Discarded) => discarded += 1,
                None => missing += 1,
            }
        }
        let intervals: Vec<f64> = shown
            .windows(2)
            .map(|w| w[1].saturating_sub(w[0]) as f64 / 1e6)
            .collect();
        println!(
            "presented {} / discarded {discarded} (never reached the screen) / no feedback {missing}; vsync flag {vsync}, zero-copy {zero_copy}",
            shown.len()
        );
        if let Some(s) = Summary::of(&intervals) {
            println!("scanout interval: {s}");
            println!(
                "  intervals > 8.33 ms: {} ({:.2} %), > 16.6 ms: {}",
                over(&intervals, 8.34),
                over(&intervals, 8.34) as f64 * 100.0 / intervals.len() as f64,
                over(&intervals, 16.7)
            );
        }
        if let Some(s) = Summary::of(&latency) {
            println!("frame start (input applied) → scanout: {s}");
        }
    }

    /// Blocks until the compositor has answered everything sent so far.
    pub fn roundtrip(&mut self) {
        let _ = self.queue.roundtrip(&mut self.state);
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
                    vsync: flags.contains(Kind::Vsync),
                    zero_copy: flags.contains(Kind::ZeroCopy),
                }
            }
            Event::Discarded => Presented::Discarded,
            _ => return,
        };
        state.presented.insert(*frame, p);
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
            state.tablet.pads += 1;
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
            state.tablet.tablets.push(name);
        }
    }
}

impl Dispatch<ZwpTabletToolV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_tablet_tool_v2::{Event, Type};
        let log = &mut state.tablet;
        match event {
            Event::Type { tool_type } => {
                if matches!(tool_type, WEnum::Value(Type::Eraser)) {
                    log.eraser_seen = true;
                }
                log.tools.push(format!("{tool_type:?}"));
            }
            Event::Motion { .. } => state.pending_motion = true,
            Event::Pressure { pressure } => {
                let p = pressure.min(u16::MAX as u32) as u16;
                log.pressure_min = Some(log.pressure_min.map_or(p, |m| m.min(p)));
                log.pressure_max = Some(log.pressure_max.map_or(p, |m| m.max(p)));
            }
            Event::Tilt { .. } => log.tilt_seen = true,
            Event::Frame { .. } if std::mem::take(&mut state.pending_motion) => {
                log.motion_frames += 1;
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
