//! Embeds every `themes/*.toml` into the crate so built-in themes ship inside
//! each binary. Adding a built-in theme is just dropping a file in `themes/`.

use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let themes_dir = manifest_dir.join("themes");
    println!("cargo:rerun-if-changed={}", themes_dir.display());

    let mut files: Vec<PathBuf> = fs::read_dir(&themes_dir)
        .expect("themes directory exists")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();

    let mut out = String::from("pub(crate) const BUILTIN: &[(&str, &str)] = &[\n");
    for path in &files {
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf-8 theme file name");
        writeln!(
            out,
            "    ({id:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .expect("write to string");
    }
    out.push_str("];\n");

    let dest =
        PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo")).join("builtin_themes.rs");
    fs::write(dest, out).expect("write generated theme table");
}
