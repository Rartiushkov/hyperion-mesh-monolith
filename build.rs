fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc_path);

    println!("cargo:rerun-if-changed=proto/hyperion_gate.proto");

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/hyperion_gate.proto"], &["proto"])?;

    Ok(())
}
