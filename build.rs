use std::{env, error::Error, path::PathBuf};

use naga::{
    back::spv,
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    // Compile shader for embedding in the program.
    // Prevents having to include an entire shader compiler...

    let shader_text_path = manifest_dir.join("src/shader.wgsl");
    let shader_text = std::fs::read_to_string(shader_text_path)?;
    let shader_module = wgsl::parse_str(&shader_text)?;

    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let shader_module_info = validator.validate(&shader_module)?;

    let shader_spirv = spv::write_vec(
        &shader_module,
        &shader_module_info,
        &spv::Options::default(),
        None,
    )?;

    let shader_spirv_bytes: &[u8] = bytemuck::cast_slice(&shader_spirv);

    let shader_spirv_path = out_dir.join("shader.spv");

    std::fs::write(&shader_spirv_path, shader_spirv_bytes)?;

    println!(
        "cargo:rustc-env=SHADER_SPV_PATH={}",
        shader_spirv_path.canonicalize()?.to_string_lossy()
    );

    Ok(())
}
