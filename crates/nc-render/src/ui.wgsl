// Interface chrome in logical px, origin top-left. Flat: rounded rectangles with a 1 px border, images
// (sketch thumbnails) clipped to a radius. No shadows, no blur (DESIGN.md).

struct View {
    size: vec2f,       // logical px
    px_per_pt: f32,
    _pad: f32,
}
@group(0) @binding(0) var<uniform> view: View;

fn to_clip(p: vec2f) -> vec4f {
    return vec4f(p.x / view.size.x * 2.0 - 1.0, 1.0 - p.y / view.size.y * 2.0, 0.0, 1.0);
}

// Signed distance to a rounded box centred on the origin.
fn rounded_box(p: vec2f, half: vec2f, r: f32) -> f32 {
    let q = abs(p) - half + vec2f(r);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

struct Rect {
    @location(0) rect: vec4f,      // x, y, w, h
    @location(1) fill: vec4f,
    @location(2) border: vec4f,
    @location(3) shape: vec2f,     // corner radius, border width
}

struct RectOut {
    @builtin(position) pos: vec4f,
    @location(0) local: vec2f,     // from the rect's centre, logical px
    @location(1) half: vec2f,
    @location(2) fill: vec4f,
    @location(3) border: vec4f,
    @location(4) shape: vec2f,
}

const CORNERS = array<vec2f, 6>(
    vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    vec2f(0.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0),
);

@vertex fn vs_rect(@builtin(vertex_index) vi: u32, r: Rect) -> RectOut {
    let margin = 1.0 / view.px_per_pt;     // room for the anti-aliased edge
    let c = CORNERS[vi];
    let p = r.rect.xy - vec2f(margin) + c * (r.rect.zw + vec2f(2.0 * margin));
    var o: RectOut;
    o.pos = to_clip(p);
    o.half = r.rect.zw * 0.5;
    o.local = p - (r.rect.xy + o.half);
    o.fill = r.fill;
    o.border = r.border;
    o.shape = r.shape;
    return o;
}

@fragment fn fs_rect(i: RectOut) -> @location(0) vec4f {
    let d = rounded_box(i.local, i.half, i.shape.x) * view.px_per_pt;   // device px
    let outside = clamp(d + 0.5, 0.0, 1.0);
    let inner = clamp(d + i.shape.y * view.px_per_pt + 0.5, 0.0, 1.0); // 1 inside the border band
    let color = mix(i.fill, i.border, inner * step(0.001, i.shape.y));
    return vec4f(color.rgb, color.a * (1.0 - outside));
}

// ---- images ----

@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct Img {
    @location(0) rect: vec4f,
    @location(1) radius: f32,
}

struct ImgOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
    @location(1) local: vec2f,
    @location(2) half: vec2f,
    @location(3) radius: f32,
}

@vertex fn vs_image(@builtin(vertex_index) vi: u32, r: Img) -> ImgOut {
    let c = CORNERS[vi];
    let p = r.rect.xy + c * r.rect.zw;
    var o: ImgOut;
    o.pos = to_clip(p);
    o.uv = c;
    o.half = r.rect.zw * 0.5;
    o.local = p - (r.rect.xy + o.half);
    o.radius = r.radius;
    return o;
}

@fragment fn fs_image(i: ImgOut) -> @location(0) vec4f {
    let d = rounded_box(i.local, i.half, i.radius) * view.px_per_pt;
    let color = textureSample(image, image_sampler, i.uv);
    return vec4f(color.rgb, color.a * clamp(0.5 - d, 0.0, 1.0));
}
