use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let extractor = "../tools/extract-assets";
    // The Steam UI bundle is NOT shipped in this repository: it is read from the local Steam install
    // (DECKSHELL_STEAM, default ~/.local/share/Steam) or from DECKSHELL_SP_JS.
    let steam_root = std::env::var_os("DECKSHELL_STEAM").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").expect("set DECKSHELL_STEAM to a local Steam install")).join(".local/share/Steam")
    });
    let bundle_path = std::env::var_os("DECKSHELL_SP_JS").map(PathBuf::from).unwrap_or_else(|| steam_root.join("steamui/sp.js"));
    let bundle = bundle_path.to_str().expect("non-UTF-8 path");
    println!("cargo:rerun-if-env-changed=DECKSHELL_SP_JS");
    for file in ["internet.mjs", "lib.mjs", "sandbox.mjs"] {
        println!("cargo:rerun-if-changed={extractor}/{file}");
    }
    println!("cargo:rerun-if-changed={bundle}");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let status = Command::new("node")
        .arg(format!("{extractor}/internet.mjs"))
        .arg(bundle)
        .arg(&out)
        .status()
        .expect("Node.js is required to extract the Internet SVGs from the Steam UI bundle");
    assert!(status.success(), "Steam Internet SVG extraction failed");
    cache_controller_glyphs(&out);

    let cfg = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/appwindow.slint", cfg).unwrap();
}

fn cache_controller_glyphs(out: &Path) {
    for name in ["DECKSHELL_STEAM", "HOME"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let steam = std::env::var_os("DECKSHELL_STEAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("HOME").expect("set DECKSHELL_STEAM to a local Steam install"),
            )
            .join(".local/share/Steam")
        });
    let source = steam.join("controller_base/images/api");
    let mut files = Vec::new();
    for variant in ["dark", "knockout", "light"] {
        let dir = source.join(variant);
        println!("cargo:rerun-if-changed={}", dir.display());
        let target = out.join("controller-glyphs").join(variant);
        std::fs::create_dir_all(&target).unwrap();
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|err| {
            panic!("Cannot read Steam glyphs at {}: {err}. Set DECKSHELL_STEAM to a local Steam install.", dir.display());
        });
        let previous_count = files.len();
        for entry in entries {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !(name.starts_with("sd_") || name.starts_with("shared_")) {
                continue;
            }
            let (style, glyph) = if let Some(glyph) = name.strip_suffix("_md.png") {
                (format!("{variant}.md"), glyph)
            } else if let Some(glyph) = name.strip_suffix(".svg").filter(|_| variant == "knockout")
            {
                (variant.to_owned(), glyph)
            } else {
                continue;
            };
            std::fs::copy(entry.path(), target.join(name)).unwrap();
            files.push(format!(
                "    ({style:?}, {glyph:?}) => Some(include_bytes!({:?}).as_slice()),\n",
                format!("controller-glyphs/{variant}/{name}")
            ));
        }
        assert!(
            files.len() > previous_count,
            "No Steam controller glyphs in {}",
            dir.display()
        );
    }
    files.sort_unstable();
    std::fs::write(out.join("controller_glyphs.rs"), format!(
        "fn controller_glyph_bytes(variant: &str, name: &str) -> Option<&'static [u8]> {{\nmatch (variant, name) {{\n{}    _ => None,\n}}\n}}\n",
        files.concat(),
    )).unwrap();
}
