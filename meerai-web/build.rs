use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=MEERAI_WEB_DIR");

    let web_dir = resolve_web_dir();
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    let out_file = out_dir.join("embedded_bundle.rs");

    println!("cargo:rerun-if-changed={}", web_dir.display());

    if has_bundle(&web_dir) {
        let mut files = Vec::new();
        collect_files(&web_dir, &web_dir, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut code = String::from(
            "pub fn embedded_asset(path: &str) -> Option<&'static [u8]> {\n    match path {\n",
        );
        for (rel, abs) in &files {
            println!("cargo:rerun-if-changed={}", abs.display());
            code.push_str(&format!(
                "        {rel:?} => Some(include_bytes!({:?})),\n",
                abs.to_string_lossy()
            ));
        }
        code.push_str("        _ => None,\n    }\n}\n");
        write_if_changed(&out_file, &code);
    } else {
        let code = "pub fn embedded_asset(_path: &str) -> Option<&'static [u8]> {\n    None\n}\n";
        write_if_changed(&out_file, code);
    }
}

fn resolve_web_dir() -> PathBuf {
    if let Some(dir) = env::var_os("MEERAI_WEB_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crate dir has workspace parent")
        .join("target")
        .join("web")
}

fn has_bundle(dir: &Path) -> bool {
    dir.join("index.html").is_file()
        && dir.join("app.js").is_file()
        && dir.join("app_bg.wasm").is_file()
}

fn is_excluded(name: &str) -> bool {
    matches!(name, ".next" | ".stamp" | "reload.js")
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_excluded(name) {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel_posix = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel_posix, path));
        }
    }
}

fn write_if_changed(path: &Path, content: &str) {
    if fs::read_to_string(path).ok().as_deref() == Some(content) {
        return;
    }
    fs::write(path, content).expect("write embedded_bundle.rs");
}
