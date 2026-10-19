fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/rstorrent.proto");

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);

    tonic_prost_build::configure()
        .out_dir(&out_dir)
        .compile_protos(&["proto/rstorrent.proto"], &["proto"])?;

    let generated = out_dir.join("rstorrent.v1.rs");
    if generated.exists() {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
        let dest = std::path::Path::new(&manifest_dir).join("src").join("proto.rs");
        std::fs::copy(&generated, &dest)?;
    }

    Ok(())
}
