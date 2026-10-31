struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0)
var tex: texture_2d<f32>;
@group(0) @binding(1)
var samp: sampler;


struct UniformBufferData {
    camera_mat: mat3x3<f32>,
    camera_perspective_correction: u32,
    image_scale: vec2<f32>
};

@group(1) @binding(0)
var<uniform> uniform_buf: UniformBufferData;


@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(view_index) view: u32) -> VertexOutput {
    var out: VertexOutput;

    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(3.0,  1.0),
    );

    out.position = vec4(positions[vertex_index], 0.0, 1.0);

    var perspective_offsets = array<vec2<f32>, 2>(
        vec2<f32>(-0.075, 0.0),
        vec2<f32>(0.075, 0.0)
    );

    let possible_perspective_offset = select(vec2(0, 0), perspective_offsets[view], uniform_buf.camera_perspective_correction > 0);

    out.uv = positions[vertex_index] / 2;                     // Apply scale correction (maps view -0.5 to 0.5)
    out.uv += possible_perspective_offset;                    // Apply fake perspective
    out.uv = (uniform_buf.camera_mat * vec3(out.uv, 1.0)).xy; // Apply camera movement
    out.uv /= uniform_buf.image_scale;                        // Apply image aspect ratio
    
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
