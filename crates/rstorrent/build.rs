//! Embed the built Web UI (`ui/dist`) into the binary at compile time.
//!
//! Generates `$OUT_DIR/ui_assets.rs`, a static asset table consumed by
//! `webui::embedded`. When `ui/dist/index.html` is absent the table is empty and
//! `UI_EMBEDDED` is false, so the server falls back to the legacy embedded
//! `index.html`. Builds without a frontend build step therefore keep working.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = env::var("RSTORRENT_UI_DIST")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../ui/dist"));

    println!("cargo:rerun-if-env-changed=RSTORRENT_UI_DIST");
    // Directory-level tracking catches Trunk's hashed filenames (adds/deletes)
    // and reruns when the directory appears or disappears.
    println!("cargo:rerun-if-changed={}", dist.display());

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ui_assets.rs");
    let index = dist.join("index.html");

    if !index.is_file() {
        fs::write(
            &out,
            "pub const UI_EMBEDDED: bool = false;\n\
             pub const INDEX_HTML: &[u8] = b\"\";\n\
             pub static ASSETS: &[(&str, &[u8], &str)] = &[];\n",
        )
        .unwrap();
        return;
    }

    let dist_abs = dist.canonicalize().unwrap();
    let index_abs = index.canonicalize().unwrap();
    let mut entries = String::new();
    collect(&dist_abs, &dist_abs, &mut entries);

    let code = format!(
        "pub const UI_EMBEDDED: bool = true;\n\
         pub const INDEX_HTML: &[u8] = include_bytes!({index:?});\n\
         pub static ASSETS: &[(&str, &[u8], &str)] = &[\n{entries}];\n",
        index = index_abs.to_string_lossy(),
        entries = entries,
    );
    fs::write(&out, code).unwrap();
}

/// Recursively append one `ASSETS` entry per file under `dir`.
fn collect(root: &Path, dir: &Path, entries: &mut String) {
    let mut children: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    for path in children {
        if path.is_dir() {
            collect(root, &path, entries);
            continue;
        }
        let rel = path.strip_prefix(root).unwrap();
        let key = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
        let abs = path.canonicalize().unwrap();
        let ctype = content_type(&path);
        entries.push_str(&format!(
            "    ({key:?}, include_bytes!({abs:?}), {ctype:?}),\n",
            key = key,
            abs = abs.to_string_lossy(),
            ctype = ctype,
        ));
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
