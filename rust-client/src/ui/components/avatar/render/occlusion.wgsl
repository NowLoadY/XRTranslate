// Small contact shadows follow the rendered geometry, including moving hair and clothing.
struct Camera {
    view_projection: mat4x4<f32>,
    light_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var color: texture_2d<f32>;
@group(1) @binding(1) var depth: texture_depth_multisampled_2d;

@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn world(pixel: vec2<i32>, dimensions: vec2<f32>) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + 0.5) / dimensions;
    let z = textureLoad(depth, pixel, 0);
    let position = camera.inverse_view_projection * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    return position.xyz / position.w;
}

@fragment fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let rgba = textureLoad(color, pixel, 0);
    let dimensions = vec2<f32>(textureDimensions(color));
    let point = world(pixel, dimensions);
    // Wider, one-sided differences suppress triangle noise and avoid borrowing
    // a normal from the other side of a silhouette or an overlapping strand.
    let right = world(min(pixel + vec2<i32>(2, 0), vec2<i32>(dimensions) - 1), dimensions) - point;
    let left = point - world(max(pixel - vec2<i32>(2, 0), vec2<i32>(0)), dimensions);
    let down = world(min(pixel + vec2<i32>(0, 2), vec2<i32>(dimensions) - 1), dimensions) - point;
    let up = point - world(max(pixel - vec2<i32>(0, 2), vec2<i32>(0)), dimensions);
    let dx = select(left, right, dot(right, right) < dot(left, left)) * 0.5;
    let dy = select(up, down, dot(down, down) < dot(up, up)) * 0.5;
    let normal = normalize(cross(dy, dx));
    if rgba.a < 0.99 { return rgba; }
    let radius_pixels = max(dimensions.x * 0.025, 2.0);
    let radius = min(length(dx), length(dy)) * radius_pixels;
    var blocked = 0.0;
    for (var i = 0; i < 24; i++) {
        let angle = f32(i) * 2.399963;
        let distance = sqrt((f32(i) + 0.5) / 24.0) * radius_pixels;
        let sample_pixel = clamp(pixel + vec2<i32>(vec2<f32>(cos(angle), sin(angle)) * distance), vec2<i32>(0), vec2<i32>(dimensions) - 1);
        let delta = world(sample_pixel, dimensions) - point;
        let length_squared = dot(delta, delta);
        let facing = max(dot(normal, delta) * inverseSqrt(max(length_squared, 0.0000001)) - 0.08, 0.0);
        let falloff = 1.0 - smoothstep(radius * radius * 0.25, radius * radius, length_squared);
        blocked += facing * falloff * textureLoad(color, sample_pixel, 0).a;
    }
    let shade = 1.0 - min(blocked / 24.0 * 1.8, 0.25);
    // Warm attenuation keeps pale skin and blonde hair from acquiring gray creases.
    return vec4<f32>(rgba.rgb * pow(vec3<f32>(shade), vec3<f32>(0.90, 1.04, 1.15)), rgba.a);
}
