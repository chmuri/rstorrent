fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/rstorrent.proto");

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);

    tonic_prost_build::configure()
        .out_dir(&out_dir)
        .compile_protos(&["proto/rstorrent.proto"], &["proto"])?;

    // `prost-build` names the generated module after the protobuf package
    // (`rtorrent.v1` -> `rtorrent.v1.rs`). Older revisions of the package used
    // `rstorrent.v1`, so accept either name before falling back to the only
    // Rust file the compiler produced.
    let generated = ["rtorrent.v1.rs", "rstorrent.v1.rs"]
        .iter()
        .map(|name| out_dir.join(name))
        .find(|path| path.exists())
        .or_else(|| {
            std::fs::read_dir(&out_dir).ok().and_then(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .find(|path| path.extension().is_some_and(|ext| ext == "rs"))
            })
        });

    if let Some(generated) = generated {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
        let dest = std::path::Path::new(&manifest_dir)
            .join("src")
            .join("proto.rs");
        std::fs::copy(&generated, &dest)?;
    }

    Ok(())
}
