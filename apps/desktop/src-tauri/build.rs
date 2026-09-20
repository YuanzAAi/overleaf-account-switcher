use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../../chrome_extension")
        .canonicalize()?;
    println!("cargo:rerun-if-changed={}", root.display());
    let mut entries = fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut assets = String::from("const EXTENSION_ASSETS: &[(&str, &[u8])] = &[\n");
    for entry in entries {
        if !entry.file_type()?.is_file() {
            return Err(io::Error::other("extension assets must be regular files"));
        }
        assets.push_str(&format!(
            "({:?}, include_bytes!({:?})),\n",
            entry.file_name().to_string_lossy(),
            entry.path(),
        ));
    }
    assets.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("extension_assets.rs"),
        assets,
    )?;
    tauri_build::build();
    Ok(())
}
