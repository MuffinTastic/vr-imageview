struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0)
var tex: texture_2d<f32>;
@group(0) @binding(1)
var samp: sampler;


struct CameraUniform {
    mat: mat3x3<f32>,
    perspective_correction: u32,
    image_scale: vec2<f32>
};

@group(1) @binding(0)
var<uniform> camera: CameraUniform;


@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(view_index) view: u32) -> VertexOutput {
    var out: VertexOutput;

    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0,  1.0),
        vec2<f32>(-1.0, -3.0),
    );

    out.position = vec4(positions[vertex_index], 0.0, 1.0) * vec4(1, -1, 0, 1);

    var perspective_offsets = array<vec2<f32>, 2>(
        vec2<f32>(-0.15, 0.0),
        vec2<f32>(0.15, 0.0)
    );

    let possible_perspective_offset = select(vec2(0, 0), perspective_offsets[view], camera.perspective_correction > 0);

    out.uv = (camera.mat * vec3((positions[vertex_index]) + possible_perspective_offset, 1.0)).xy / camera.image_scale;
    
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
