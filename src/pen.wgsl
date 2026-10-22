// Instanced round-capped lines. Rasterize a tight, oriented capsule rectangle
// rather than visiting every pixel in a diagonal line's bounding box on the CPU.
struct Line {
    @location(0) endpoints: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) radius: f32,
};
struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) endpoints: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) radius: f32,
};
@vertex fn vs_main(line: Line, @builtin(vertex_index) index: u32) -> Varyings {
    let corners = array<vec2<f32>, 6>(
        vec2(0., -1.), vec2(1., -1.), vec2(0., 1.),
        vec2(0., 1.), vec2(1., -1.), vec2(1., 1.));
    let delta = line.endpoints.zw - line.endpoints.xy;
    let distance = length(delta);
    let tangent = delta / max(distance, 0.00001);
    let direction = select(vec2(1., 0.), tangent, distance > 0.00001);
    let normal = vec2(-direction.y, direction.x);
    let extent = line.radius + 0.5;
    let corner = corners[index];
    let pixel = line.endpoints.xy + direction * (corner.x * (distance + 2. * extent) - extent)
        + normal * corner.y * extent;
    var out: Varyings;
    out.position = vec4(pixel.x / 240. - 1., 1. - pixel.y / 180., 0., 1.);
    out.endpoints = line.endpoints;
    out.color = line.color;
    out.radius = line.radius;
    return out;
}
@fragment fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let delta = in.endpoints.zw - in.endpoints.xy;
    let offset = in.position.xy - in.endpoints.xy;
    let t = clamp(dot(offset, delta) / max(dot(delta, delta), 0.000001), 0., 1.);
    let coverage = clamp(in.radius + 0.5 - length(offset - t * delta), 0., 1.);
    let alpha = in.color.a * coverage;
    return vec4(in.color.rgb * alpha, alpha);
}
