//! Synthetic pointer input for `window input`: a virtual pointer (`zwlr_virtual_pointer_v1`) moving in a
//! circle. It goes through the compositor's normal input path, `wl_pointer.frame` included, and each
//! event carries our CLOCK_MONOTONIC ms timestamp, so the bench can measure event → scanout.
//! (Hyprland 0.56's `cursor.move` warp sends motion without `frame`, which winit never delivers.)
//! With `strokes`, the left button is held for each half loop, drawing arcs.
//!
//! `script` drives a scripted session instead (the Phase 3 recording), one command per stdin line,
//! positions in output px:
//!   `move X Y MS`  glide there over MS ms at 250 Hz     `down` / `up`  left button
//!   `wait MS`                                            `key ARGS…`    run `wtype ARGS…` and wait
//!   `hold MOD MS`  hold a modifier (wtype) for MS ms while the script continues

use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

struct State;

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

wayland_client::delegate_noop!(State: ignore ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ignore ZwlrVirtualPointerV1);

/// Output size the absolute motion maps onto (the reference monitor).
const EXTENT: (u32, u32) = (2560, 1440);

/// evdev BTN_LEFT.
const BTN_LEFT: u32 = 0x110;

fn connect() -> (
    Connection,
    wayland_client::EventQueue<State>,
    ZwlrVirtualPointerV1,
) {
    let conn = Connection::connect_to_env().expect("connect to the Wayland display");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .expect("compositor has no zwlr_virtual_pointer_manager_v1");
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    queue.roundtrip(&mut State).expect("roundtrip");
    (conn, queue, pointer)
}

fn now_ms() -> u32 {
    (nc_wayland::mono_ns() / 1_000_000) as u32
}

pub fn script(input: impl std::io::BufRead) {
    let (conn, mut queue, pointer) = connect();
    let mut at = (EXTENT.0 as f64 / 2.0, EXTENT.1 as f64 / 2.0);
    let send = |f: &dyn Fn(u32)| {
        f(now_ms());
        pointer.frame();
        conn.flush().expect("flush");
    };
    let mut holds = Vec::new();
    for line in input.lines() {
        let line = line.expect("read script");
        let words: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> f64 { words[i].parse().expect("number") };
        match words.first().copied() {
            None | Some("#") => {}
            Some("move") => {
                let (to, ms) = ((num(1), num(2)), num(3));
                let steps = (ms / 4.0).ceil().max(1.0) as u32;
                let from = at;
                for i in 1..=steps {
                    let k = i as f64 / steps as f64;
                    at = (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k);
                    send(&|t| {
                        pointer.motion_absolute(t, at.0 as u32, at.1 as u32, EXTENT.0, EXTENT.1)
                    });
                    std::thread::sleep(Duration::from_millis(4));
                }
            }
            Some(b @ ("down" | "up")) => {
                let state = if b == "down" {
                    wl_pointer::ButtonState::Pressed
                } else {
                    wl_pointer::ButtonState::Released
                };
                send(&|t| pointer.button(t, BTN_LEFT, state));
            }
            Some("wait") => std::thread::sleep(Duration::from_secs_f64(num(1) / 1e3)),
            Some("key") => {
                let ok = std::process::Command::new("wtype")
                    .args(&words[1..])
                    .status()
                    .is_ok_and(|s| s.success());
                assert!(ok, "wtype {:?} failed", &words[1..]);
            }
            Some("hold") => holds.push(
                std::process::Command::new("wtype")
                    .args(["-M", words[1], "-s", words[2], "-m", words[1]])
                    .spawn()
                    .expect("run wtype"),
            ),
            Some(other) => panic!("unknown script command {other:?}"),
        }
    }
    for mut h in holds {
        let _ = h.wait();
    }
    pointer.destroy();
    let _ = queue.roundtrip(&mut State);
}

pub fn run(secs: f64, hz: f64, strokes: bool) {
    let (conn, mut queue, pointer) = connect();
    let (cx, cy, radius) = (EXTENT.0 as f64 / 2.0, EXTENT.1 as f64 / 2.0, 300.0);
    let start = Instant::now();
    let mut sent = 0u64;
    let mut held = false;
    while start.elapsed().as_secs_f64() < secs {
        let t = start.elapsed().as_secs_f64();
        let a = t * std::f64::consts::PI; // one loop every 2 s
        let (x, y) = (cx + radius * a.cos(), cy + radius * a.sin());
        let time_ms = (nc_wayland::mono_ns() / 1_000_000) as u32;
        pointer.motion_absolute(time_ms, x as u32, y as u32, EXTENT.0, EXTENT.1);
        // Draw for 0.7 s of every second: separate arcs, each its own stroke.
        let want = strokes && t.fract() < 0.7 && t < secs - 0.1;
        if want != held {
            let state = if want {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            };
            pointer.button(time_ms, BTN_LEFT, state);
            held = want;
        }
        pointer.frame();
        conn.flush().expect("flush");
        sent += 1;
        let next = start + Duration::from_secs_f64(sent as f64 / hz);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    if held {
        let time_ms = (nc_wayland::mono_ns() / 1_000_000) as u32;
        pointer.button(time_ms, BTN_LEFT, wl_pointer::ButtonState::Released);
        pointer.frame();
    }
    pointer.destroy();
    let _ = queue.roundtrip(&mut State);
    println!(
        "sent {sent} pointer frames in {secs:.0} s ({:.0}/s)",
        sent as f64 / secs
    );
}
