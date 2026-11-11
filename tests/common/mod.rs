use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn fixture(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/fixtures")
        .join(name);
    assert!(
        path.is_file(),
        "missing fixture {}; run just fixtures with a local Go toolchain",
        path.display()
    );
    path
}

// Shared by the CLI-level test targets; decoder-level targets compile this
// module without using every helper.
#[allow(dead_code)]
pub fn run_json(path: &Path) -> (Output, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_goatlas"))
        .arg(path)
        .env("PATH", "")
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout).expect("scan must emit JSON");
    (out, json)
}
