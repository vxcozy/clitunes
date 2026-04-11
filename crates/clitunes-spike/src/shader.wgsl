struct Uniforms {
    width: f32,
    height: f32,
    time: f32,
    frame: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VertexOutput {
    // Fullscreen triangle (3 vertices, no buffer needed)
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

// Deterministic test pattern: rotating angular gradient + bouncing rectangle.
// Frame-to-frame diff is non-zero so terminal can't no-op the redraw.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv; // 0..1
    let centred = uv - vec2<f32>(0.5, 0.5);

    // Rotating angular gradient
    let angle = atan2(centred.y, centred.x);
    let radius = length(centred);
    let rot = u.time * 0.6;
    let hue = fract((angle + rot) / 6.2831853 + radius * 0.5);

    // Cheap HSV→RGB
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let p = abs(fract(vec3<f32>(hue) + k / 6.0) * 6.0 - vec3<f32>(3.0));
    let base = clamp(p - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0));

    // Bouncing rectangle so frame diff is provably non-zero
    let bx = 0.5 + 0.4 * sin(u.time * 1.7);
    let by = 0.5 + 0.4 * cos(u.time * 2.3);
    let in_box = step(abs(uv.x - bx), 0.06) * step(abs(uv.y - by), 0.06);
    let box_color = vec3<f32>(1.0, 1.0, 1.0);

    let color = mix(base, box_color, in_box);
    return vec4<f32>(color, 1.0);
}
