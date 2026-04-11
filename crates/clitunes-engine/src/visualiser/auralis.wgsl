// Auralis slice-1: 64 bars + warm-to-cool gradient + tiny glow.

struct Uniforms {
    width: f32,
    height: f32,
    time: f32,
    num_bars: f32,
    bars: array<vec4<f32>, 16>, // 64 f32 packed as 16 vec4
};

@group(0) @binding(0) var<uniform> u: Uniforms;

fn bar_height(idx: u32) -> f32 {
    let v = u.bars[idx / 4u];
    let sub = idx % 4u;
    if sub == 0u { return v.x; }
    if sub == 1u { return v.y; }
    if sub == 2u { return v.z; }
    return v.w;
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VertexOutput;
    out.clip_position = vec4<f32>(positions[vidx], 0.0, 1.0);
    out.uv = uvs[vidx];
    return out;
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let p = abs(fract(vec3<f32>(h) + k / 6.0) * 6.0 - vec3<f32>(3.0));
    return v * mix(vec3<f32>(1.0), clamp(p - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0)), s);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let n = u.num_bars;
    let bar_w = 1.0 / n;
    let gap = bar_w * 0.15;

    // Which bar is this fragment in?
    let bar_idx = clamp(floor(uv.x * n), 0.0, n - 1.0);
    let bar_u = fract(uv.x * n);
    let in_bar = step(gap * 0.5, bar_u) * step(bar_u, 1.0 - gap * 0.5);

    // Bar height, from the bottom up.
    let h = bar_height(u32(bar_idx));
    let bar_top = 0.08 + h * 0.85;
    let in_fill = step(uv.y, bar_top) * in_bar;

    // Background: soft radial vignette + subtle hue wash.
    let centre = uv - vec2<f32>(0.5, 0.5);
    let r = length(centre);
    let bg_hue = 0.55 + 0.05 * sin(u.time * 0.25);
    let bg = hsv_to_rgb(bg_hue, 0.6, 0.06) * (1.0 - r * 0.9);

    // Bar colour: warm at bottom → cool at top, with energy-based saturation.
    let local_y = uv.y / max(bar_top, 0.001);
    let hue = mix(0.06, 0.62, local_y);
    let sat = 0.55 + 0.45 * h;
    let val = mix(0.45, 1.15, h);
    let bar_col = hsv_to_rgb(hue, sat, val);

    // Glow line on top of each bar.
    let top_glow = smoothstep(0.0, 0.015, bar_top - uv.y) * smoothstep(0.0, 0.015, uv.y - (bar_top - 0.018));
    let glow_col = vec3<f32>(1.0, 0.95, 0.85) * top_glow * h * 1.5;

    let color = bg + bar_col * in_fill + glow_col * in_bar;
    return vec4<f32>(color, 1.0);
}
