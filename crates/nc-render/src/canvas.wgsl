// World space is y-down, origin top-left (docs/PLAN.md §2). The only flip happens here, into clip space.

struct View {
    offset: vec2f,     // logical px
    scale: f32,        // zoom
    px_per_pt: f32,    // surface scale factor (fractional on Wayland)
    size: vec2f,       // logical px, of the pane being drawn
    origin: vec2f,     // the pane's top-left, physical px (side-by-side has two panes)
    lift_abcd: vec4f,  // affine for lifted segments (a selection being moved), kurbo coefficient order
    lift_ef: vec2f,
    alpha: vec2f,      // opacity of the raw sketch and of the wireframe
    background: vec4f, // linear RGB, w = 1 for the light theme
    grid: vec4f,       // grid line colour, w = 1 to draw the grid
}

// Light theme: lightness inverted, hue kept (nc_core::theme::ink_on_light). In sRGB: c + 1 - 2Y.
fn ink_on_light(c: vec3f) -> vec3f {
    let s = select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
    let y = dot(s, vec3f(0.299, 0.587, 0.114));
    let t = clamp(s + vec3f(1.0 - 2.0 * y), vec3f(0.0), vec3f(1.0));
    return select(pow((t + 0.055) / 1.055, vec3f(2.4)), t / 12.92, t <= vec3f(0.04045));
}

// SegmentInstance.flags
const LIFTED: u32 = 1u;
const WIRE: u32 = 2u;
const CHROME: u32 = 4u;
const INK_ON_TOP: u32 = 8u;
const FILL: u32 = 16u;     // a filled rounded box from a to b, half_width = corner radius
const ELLIPSE: u32 = 32u;  // with FILL: the ellipse inscribed in that box

fn lift(p: vec2f) -> vec2f {
    let m = view.lift_abcd;
    return vec2f(m.x * p.x + m.z * p.y, m.y * p.x + m.w * p.y) + view.lift_ef;
}
@group(0) @binding(0) var<uniform> view: View;

fn world_to_clip(p: vec2f) -> vec4f {
    let s = p * view.scale + view.offset;
    return vec4f(s.x / view.size.x * 2.0 - 1.0, 1.0 - s.y / view.size.y * 2.0, 0.0, 1.0);
}

// ---- grid: one full-screen triangle, lines computed per pixel from the inverse view ----

@vertex fn vs_grid(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn grid_line(world: vec2f, spacing: f32) -> f32 {
    let px = abs(fract(world / spacing - 0.5) - 0.5) * spacing * view.scale * view.px_per_pt;
    return 1.0 - clamp(min(px.x, px.y) - 0.5, 0.0, 1.0);
}

@fragment fn fs_grid(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let world = ((frag.xy - view.origin) / view.px_per_pt - view.offset) / view.scale;
    let level = log2(view.scale) / 2.0;           // spacing steps by 4x per level
    let spacing = 32.0 / pow(4.0, floor(level));
    let fade = 1.0 - fract(level);                // minor lines fade out as they get dense
    let minor = grid_line(world, spacing) * mix(0.25, 1.0, fade);
    let major = grid_line(world, spacing * 4.0);
    let bg = view.background.rgb;
    let ink = view.grid.rgb;
    return vec4f(mix(bg, ink, max(minor * 0.5, major) * view.grid.w), 1.0);
}

// ---- strokes: one instanced quad per segment, shaded as an SDF capsule (round caps and joins, AA at any angle) ----

struct Seg {
    @location(0) a: vec2f,
    @location(1) b: vec2f,
    @location(2) half_width: f32,
    @location(3) color: vec4f,
    @location(4) flags: u32,
}

struct SegOut {
    @builtin(position) pos: vec4f,
    @location(0) world: vec2f,
    @location(1) a: vec2f,
    @location(2) b: vec2f,
    @location(3) half_width: f32,
    @location(4) color: vec4f,
    @location(5) @interpolate(flat) flags: u32,
}

@vertex fn vs_seg(@builtin(vertex_index) vi: u32, s: Seg) -> SegOut {
    var corners = array<vec2f, 6>(
        vec2f(0.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(0.0, -1.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0),
    );
    let px_world = 1.0 / (view.scale * view.px_per_pt);
    let hw = max(s.half_width, 0.5 * px_world);   // never thinner than a device hairline
    var sa = s.a;
    var sb = s.b;
    if (s.flags & LIFTED) != 0u {
        sa = lift(sa);
        sb = lift(sb);
    }
    let d = sb - sa;
    let len = length(d);
    let t = select(vec2f(1.0, 0.0), d / len, len > 1e-6);
    let n = vec2f(-t.y, t.x);
    let r = hw + px_world;                        // 1 px margin for the AA ramp
    let c = corners[vi];
    var p = mix(sa - t * r, sb + t * r, c.x) + n * r * c.y;
    if (s.flags & FILL) != 0u {
        // The box itself, plus the AA margin.
        p = mix(sa - vec2f(px_world), sb + vec2f(px_world), vec2f(c.x, c.y * 0.5 + 0.5));
    }

    var o: SegOut;
    o.pos = world_to_clip(p);
    o.world = p;
    o.a = sa;
    o.b = sb;
    o.half_width = select(hw, s.half_width, (s.flags & FILL) != 0u);
    o.flags = s.flags;
    var layer = view.alpha.x;
    if (s.flags & WIRE) != 0u {
        layer = view.alpha.y;
        // A styled wireframe's fills let the sketch show through while the sketch is shown too.
        if (s.flags & FILL) != 0u {
            layer *= 1.0 - 0.5 * view.alpha.x;
        }
    } else if (s.flags & (CHROME | INK_ON_TOP)) != 0u {
        layer = 1.0;
    }
    // Only ink inverts on the light canvas; the wireframe and chrome come in theme colours.
    var rgb = s.color.rgb;
    if view.background.w > 0.5 && (s.flags & (WIRE | CHROME)) == 0u {
        rgb = ink_on_light(rgb);
    }
    o.color = vec4f(rgb, s.color.a * layer);
    return o;
}

@fragment fn fs_seg(i: SegOut) -> @location(0) vec4f {
    var dist: f32;
    if (i.flags & FILL) != 0u {
        let half = max((i.b - i.a) * 0.5, vec2f(1e-4));
        let p = i.world - (i.a + i.b) * 0.5;
        if (i.flags & ELLIPSE) != 0u {
            // Distance to an ellipse, first-order approximation (exact on circles).
            let k0 = length(p / half);
            let k1 = length(p / (half * half));
            dist = k0 * (k0 - 1.0) / max(k1, 1e-6);
        } else {
            let r = clamp(i.half_width, 0.0, min(half.x, half.y));
            let q = abs(p) - half + r;
            dist = length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
        }
    } else {
        let ab = i.b - i.a;
        let h = clamp(dot(i.world - i.a, ab) / max(dot(ab, ab), 1e-12), 0.0, 1.0);
        dist = length(i.world - i.a - ab * h) - i.half_width;
    }
    let alpha = clamp(0.5 - dist * view.scale * view.px_per_pt, 0.0, 1.0);
    return vec4f(i.color.rgb, i.color.a * alpha);
}
