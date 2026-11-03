use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../agent-rules");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut out = String::new();
    let quit = dir.join("quit");
    println!("cargo:rerun-if-changed={}", quit.display());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&quit)
        .unwrap_or_else(|e| panic!("{}: {e}", quit.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    files.sort();
    out += "const BUNDLED_QUIT: &[(&str, &str)] = &[\n";
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_stem().unwrap().to_str().unwrap();
        let full = path.canonicalize().unwrap();
        out += &format!("    ({name:?}, include_str!({:?})),\n", full.display().to_string());
    }
    out += "];\n";
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bundled.rs"), out).unwrap();
}
