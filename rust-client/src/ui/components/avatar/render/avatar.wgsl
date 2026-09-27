struct Camera { view_projection: mat4x4<f32>, light_projection: mat4x4<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var shadow_map: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) model0: vec4<f32>,
    @location(3) model1: vec4<f32>,
    @location(4) model2: vec4<f32>,
    @location(5) model3: vec4<f32>,
    @location(6) normal0: vec4<f32>,
    @location(7) normal1: vec4<f32>,
    @location(8) normal2: vec4<f32>,
    @location(9) color: vec4<f32>,
    @location(10) shape: vec4<f32>,
    @location(11) happy_position: vec3<f32>,
    @location(12) happy_normal: vec3<f32>,
    @location(13) other_position: vec3<f32>,
    @location(14) other_normal: vec3<f32>,
}
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
    @location(4) material: vec2<f32>,
}
fn transform_vertex(input: VertexInput) -> VertexOutput {
    let model = mat4x4<f32>(input.model0, input.model1, input.model2, input.model3);
    let normal = mat3x3<f32>(input.normal0.xyz, input.normal1.xyz, input.normal2.xyz);
    let point = input.position + input.happy_position * input.shape.x + input.other_position * input.shape.y;
    let morphed_normal = input.normal + input.happy_normal * input.shape.x + input.other_normal * input.shape.y;
    var output: VertexOutput;
    output.position = camera.view_projection * model * vec4<f32>(point, 1.0);
    output.world = (model * vec4<f32>(point, 1.0)).xyz;
    output.material = input.shape.zw;
    output.normal = normal * morphed_normal;
    output.color = input.color;
    return output;
}
@vertex fn model_vertex(input: VertexInput) -> VertexOutput { return transform_vertex(input); }
@vertex fn shadow_vertex(input: VertexInput) -> @builtin(position) vec4<f32> {
    let vertex = transform_vertex(input);
    return camera.light_projection * vec4<f32>(vertex.world, 1.0);
}

fn soft_shadow(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    let position = camera.light_projection * vec4<f32>(world, 1.0);
    let projected = position.xyz / position.w;
    let uv = projected.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let bias = max(0.0025 * (1.0 - dot(normal, normalize(vec3<f32>(-3.0, 5.0, 7.0)))), 0.0016);
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_map));
    var visibility = 0.0;
    // A fixed disk avoids the visible bands that a wide square PCF grid leaves below the visor.
    for (var i = 0; i < 24; i++) {
        let angle = f32(i) * 2.399963;
        let radius = sqrt((f32(i) + 0.5) / 24.0) * 10.0;
        let offset = vec2<f32>(cos(angle), sin(angle)) * radius * texel;
        visibility += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, projected.z - bias);
    }
    return visibility / 24.0;
}

// Derivative-width transitions retain flat cel tones while smoothing their moving edges.
fn cel_step(threshold: f32, value: f32) -> f32 {
    let width = max(fwidth(value), 0.012);
    return smoothstep(threshold - width, threshold + width, value);
}

@fragment fn model_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(input.normal);
    let diffuse = dot(normal, normalize(vec3<f32>(-3.0, 5.0, 7.0)));
    let bands = 0.55 * cel_step(0.02, diffuse) + 0.45 * cel_step(0.60, diffuse);
    let shadow = cel_step(0.55, soft_shadow(input.world, normal));
    let tone = 1.0 - input.material.x * (1.0 - bands * mix(0.25, 1.0, shadow));
    return vec4<f32>(input.color.rgb * tone, 1.0);
}

// Back-facing expanded geometry provides a thin, material-colored cartoon contour.
@vertex fn outline_vertex(input: VertexInput) -> VertexOutput {
    var output = transform_vertex(input);
    let world = output.world + normalize(output.normal) * input.shape.w;
    output.position = camera.view_projection * vec4<f32>(world, 1.0);
    return output;
}
@fragment fn outline_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    if input.material.y <= 0.0 { discard; }
    return vec4<f32>(input.color.rgb * 0.42, 1.0);
}
