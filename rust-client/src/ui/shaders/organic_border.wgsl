struct BorderUniform {
    size_px: vec2<f32>,
    radius_px: f32,
    half_width_px: f32,
    displacement_px: f32,
    noise_scale_px: f32,
    inset_px: f32,
    seed: f32,
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> border: BorderUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let position = positions[vertex_index];
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = position * 0.5 + vec2<f32>(0.5);
    return output;
}

fn rounded_box_sdf(point: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let corner = abs(point) - half_size + vec2<f32>(radius);
    return min(max(corner.x, corner.y), 0.0)
        + length(max(corner, vec2<f32>(0.0)))
        - radius;
}

fn hash_2d(cell: vec2<f32>) -> f32 {
    let mixed = dot(cell, vec2<f32>(127.1, 311.7));
    return fract(sin(mixed) * 43758.5453123);
}

fn smooth_noise(point: vec2<f32>) -> f32 {
    let cell = floor(point);
    let local = fract(point);
    let blend = local * local * local * (local * (local * 6.0 - vec2<f32>(15.0)) + vec2<f32>(10.0));
    let lower = mix(hash_2d(cell), hash_2d(cell + vec2<f32>(1.0, 0.0)), blend.x);
    let upper = mix(
        hash_2d(cell + vec2<f32>(0.0, 1.0)),
        hash_2d(cell + vec2<f32>(1.0, 1.0)),
        blend.x,
    );
    return mix(lower, upper, blend.y);
}

fn organic_noise(point: vec2<f32>) -> f32 {
    let seed_offset = vec2<f32>(border.seed * 1.731, border.seed * -2.417);
    let primary = smooth_noise(point + seed_offset);
    let rotated = vec2<f32>(
        point.x * 0.8 - point.y * 0.6,
        point.x * 0.6 + point.y * 0.8,
    );
    let detail = smooth_noise(rotated * 2.03 + seed_offset.yx + vec2<f32>(19.4, -7.1));
    return primary * 0.72 + detail * 0.28;
}

fn linear_from_srgb(value: vec3<f32>) -> vec3<f32> {
    let lower = value / vec3<f32>(12.92);
    let upper = pow((value + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(upper, lower, value <= vec3<f32>(0.04045));
}

fn border_alpha(uv: vec2<f32>) -> f32 {
    let point = (uv - vec2<f32>(0.5)) * border.size_px;
    let half_size = border.size_px * 0.5 - vec2<f32>(border.inset_px);
    let base_distance = rounded_box_sdf(point, half_size, border.radius_px);

    // Most fragments are far from the outline. Skip the noise work for them.
    let reach = border.displacement_px + border.half_width_px + 2.5;
    if abs(base_distance) > reach {
        return 0.0;
    }

    let noise = organic_noise(point * border.noise_scale_px);
    let distance = base_distance + (noise * 2.0 - 1.0) * border.displacement_px;
    let antialias = max(fwidth(distance), 0.75);
    return 1.0 - smoothstep(
        border.half_width_px - antialias,
        border.half_width_px + antialias,
        abs(distance),
    );
}

fn premultiplied_gamma_color(uv: vec2<f32>) -> vec4<f32> {
    let alpha = border.color.a * border_alpha(uv);
    return vec4<f32>(border.color.rgb * alpha, alpha);
}

@fragment
fn fs_main_gamma_framebuffer(input: VertexOutput) -> @location(0) vec4<f32> {
    return premultiplied_gamma_color(input.uv);
}

@fragment
fn fs_main_linear_framebuffer(input: VertexOutput) -> @location(0) vec4<f32> {
    let gamma = premultiplied_gamma_color(input.uv);
    if gamma.a <= 0.0 {
        return gamma;
    }
    let unmultiplied = gamma.rgb / gamma.a;
    return vec4<f32>(linear_from_srgb(unmultiplied) * gamma.a, gamma.a);
}
