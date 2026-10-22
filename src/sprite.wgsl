struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) shape: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) skin_size: vec2<f32>,
    @location(4) @interpolate(flat) mask: u32,
};
@group(0) @binding(0) var costume: texture_2d<f32>;
@group(0) @binding(1) var costume_sampler: sampler;
@vertex fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
    @location(2) shape: vec4<f32>, @location(3) color: vec4<f32>,
    @location(4) skin_size: vec2<f32>, @location(5) mask: u32) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.shape = shape;
    out.color = color;
    out.skin_size = skin_size;
    out.mask = mask;
    return out;
}
// Largest-component HSV conversion, including Scratch's epsilon and ties.
fn to_hsv(rgb: vec3<f32>) -> vec3<f32> {
    let high = max(rgb.r,max(rgb.g,rgb.b));
    let delta = high-min(rgb.r,min(rgb.g,rgb.b));
    let denominator = 6.0*delta+0.001;
    var hue: f32;
    if rgb.r > max(rgb.g,rgb.b) {
        if rgb.b > rgb.g { hue=1.0-(rgb.b-rgb.g)/denominator; }
        else { hue=(rgb.g-rgb.b)/denominator; }
    } else if rgb.b > rgb.g { hue=2.0/3.0+(rgb.r-rgb.g)/denominator; }
    else { hue=1.0/3.0+(rgb.b-rgb.r)/denominator; }
    return vec3<f32>(hue,delta/(high+0.001),high);
}
fn from_hsv(hsv: vec3<f32>) -> vec3<f32> {
    let sector = hsv.x*6.0;
    let chroma = hsv.z*hsv.y;
    let second = chroma*(1.0-abs((sector-2.0*floor(sector/2.0))-1.0));
    var rgb: vec3<f32>;
    switch u32(floor(sector)) % 6u {
        case 0u: { rgb=vec3<f32>(chroma,second,0.0); }
        case 1u: { rgb=vec3<f32>(second,chroma,0.0); }
        case 2u: { rgb=vec3<f32>(0.0,chroma,second); }
        case 3u: { rgb=vec3<f32>(0.0,second,chroma); }
        case 4u: { rgb=vec3<f32>(second,0.0,chroma); }
        default: { rgb=vec3<f32>(chroma,0.0,second); }
    }
    return rgb+vec3<f32>(hsv.z-chroma);
}
@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var uv = in.uv;
    if (in.mask & 16u) != 0u { uv=fract(uv*in.color.x); }
    if (in.mask & 8u) != 0u {
        let cells = in.skin_size/in.shape.w;
        uv=(floor(uv*cells)+vec2<f32>(0.5))/cells;
    }
    if (in.mask & 4u) != 0u {
        let delta=uv-vec2<f32>(0.5);
        let strength=max(1.0-2.0*length(delta),0.0);
        let angle=in.shape.z*strength*strength;
        let s=sin(angle);
        let c=cos(angle);
        uv=vec2<f32>(c*delta.x+s*delta.y,-s*delta.x+c*delta.y)+vec2<f32>(0.5);
    }
    if (in.mask & 2u) != 0u {
        let delta=(uv-vec2<f32>(0.5))*2.0;
        let distance=length(delta);
        if distance > 0.0 {
            let radius=pow(min(distance,1.0),in.shape.y)*max(distance,1.0);
            uv=vec2<f32>(0.5)+delta*(0.5*radius/distance);
        }
    }
    var rgba=textureSampleLevel(costume,costume_sampler,uv,0.0);
    if (in.mask & 33u) != 0u {
        let divisor=rgba.a+0.001;
        var rgb=clamp(rgba.rgb/divisor,vec3<f32>(0.0),vec3<f32>(1.0));
        if (in.mask & 1u) != 0u {
            var hsv=to_hsv(rgb);
            if hsv.z < 0.055 { hsv=vec3<f32>(0.0,1.0,0.055); }
            else if hsv.y < 0.09 { hsv.y=0.09; hsv.x=0.0; }
            hsv.x=fract(hsv.x+in.shape.x);
            rgb=from_hsv(hsv);
        }
        if (in.mask & 32u) != 0u { rgb=clamp(rgb+vec3<f32>(in.color.y),vec3<f32>(0.0),vec3<f32>(1.0)); }
        rgba=vec4<f32>(rgb*divisor,rgba.a);
    }
    if (in.mask & 64u) != 0u { rgba*=in.color.z; }
    return rgba;
}
