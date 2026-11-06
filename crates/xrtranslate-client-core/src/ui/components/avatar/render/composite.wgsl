@group(0) @binding(0) var avatar_texture: texture_2d<f32>;
@group(0) @binding(1) var avatar_sampler: sampler;
struct CompositeOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) opacity: f32 }
@vertex fn composite_vertex(@builtin(vertex_index) index: u32, @location(0) uv_rect: vec4<f32>, @location(1) opacity: f32) -> CompositeOutput {
    let positions = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let position = positions[index];
    let uv = position * vec2<f32>(0.5, -0.5) + 0.5;
    return CompositeOutput(vec4<f32>(position, 0.0, 1.0), uv_rect.xy + uv * uv_rect.zw, opacity);
}
@fragment fn composite_linear(input: CompositeOutput) -> @location(0) vec4<f32> {
    return textureSample(avatar_texture, avatar_sampler, input.uv) * input.opacity;
}
@fragment fn composite_gamma(input: CompositeOutput) -> @location(0) vec4<f32> {
    let sample = textureSample(avatar_texture, avatar_sampler, input.uv);
    let alpha = sample.a;
    let linear = sample.rgb / max(alpha, 0.0001);
    let gamma = select(12.92 * linear, 1.055 * pow(max(linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055, linear > vec3<f32>(0.0031308));
    return vec4<f32>(gamma * alpha, alpha) * input.opacity;
}
