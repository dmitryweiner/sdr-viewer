//! Rust types from Meshtastic's protobuf files in `proto/`, compiled with
//! `protox`, so that no `protoc` has to be installed anywhere.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let files = protox::compile(["meshtastic/mesh.proto"], ["proto"])?;
    prost_build::Config::new().compile_fds(files)?;
    Ok(())
}
