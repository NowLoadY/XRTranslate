@group(0) @binding(0) var<uniform> time: vec4<f32>;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) point: vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let point = array<vec2<f32>, 3>(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.))[index];
    return Vertex(vec4(point, 0., 1.), point);
}
@fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    let angle = time.x * 0.5;
    let p = mat2x2(vec2(cos(angle), sin(angle)), vec2(-sin(angle), cos(angle))) * input.point;
    let edge = max(abs(p.x), abs(p.y)) - 0.32;
    let coverage = 1. - smoothstep(-0.006, 0.006, edge);
    let color = mix(vec3(0.025, 0.035, 0.055), vec3(0.23, 0.48, 0.82), coverage);
    return vec4(color, 1.);
}
