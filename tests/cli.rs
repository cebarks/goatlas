mod common;

use common::{fixture, run_json};
use std::fs;
use std::process::Command;

#[test]
fn module_report_preserves_identity_as_coverage_advances() {
    let (out, j) = run_json(&fixture("app.stripped"));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(j["schema_version"], 1);
    assert_eq!(j["coverage"]["build_info"]["state"], "decoded");
    assert_eq!(j["coverage"]["functions"]["state"], "decoded");
    assert_eq!(j["coverage"]["inlining"]["state"], "unavailable");
    assert_eq!(
        j["build"]["dependencies"][0]["path"],
        "example.com/golang-symbol/fixturedep"
    );
    assert_eq!(j["build"]["dependencies"][0]["version"], "v1.2.3");
    assert_eq!(j["build"]["replacements"][0]["to"]["path"], "../dependency");
}

#[test]
fn extractor_patch_identity_is_explicit() {
    let (_, j) = run_json(&fixture("app.stripped"));
    assert_eq!(j["producer"]["extractor_version"], "1.2.0");
    assert_eq!(j["producer"]["extractor_patch"], "strict-buildinfo-v1");
    assert_eq!(j["coverage"]["build_info"]["expected"], 1);
    assert_eq!(j["coverage"]["build_info"]["visited"], 1);
    assert_eq!(j["coverage"]["build_info"]["expected_observations"], 1);
    assert_eq!(j["coverage"]["build_info"]["observations"], 1);
    assert!(
        j["coverage"]["build_info"]["issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn same_bytes_at_different_paths_produce_identical_json() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("first copy");
    let b = tmp.path().join("second copy");
    fs::copy(fixture("app.stripped"), &a).unwrap();
    fs::copy(&a, &b).unwrap();
    let (a, _) = run_json(&a);
    let (b, _) = run_json(&b);
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(a.status.code(), b.status.code());
}

#[test]
fn unreadable_or_nonregular_input_has_no_json_and_exit_1() {
    let tmp = tempfile::tempdir().unwrap();
    let big = tmp.path().join("oversized");
    fs::File::create(&big)
        .unwrap()
        .set_len((1 << 30) + 1)
        .unwrap();
    for path in [tmp.path().join("absent"), tmp.path().to_path_buf(), big] {
        let out = Command::new(env!("CARGO_BIN_EXE_golang-symbol"))
            .arg(path)
            .env("PATH", "")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert!(!out.stderr.is_empty());
    }
}

#[test]
fn usage_and_version_are_not_scan_reports() {
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_golang-symbol"))
            .args(args)
            .env("PATH", "")
            .output()
            .unwrap()
    };
    for args in [&[][..], &["a", "b"][..], &["--invalid"][..]] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
    }
    let help = run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .starts_with("Usage: golang-symbol <FILE>")
    );
    let version = run(&["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn missing_fixture_is_a_setup_failure() {
    assert!(std::panic::catch_unwind(|| fixture("__missing_fixture_control")).is_err());
}

#[test]
fn broken_go_is_a_setup_failure() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let shim = tmp.path().join("go");
    fs::write(&shim, "#!/bin/sh\nexit 19\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        tmp.path().display(),
        std::env::var("PATH").unwrap()
    );
    let out = Command::new("just")
        .arg("fixtures-base")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("local Go toolchain"));
}

#[test]
fn scan_does_not_execute_the_target() {
    let tmp = tempfile::tempdir().unwrap();
    let sentinel = tmp.path().join("executed");
    let out = Command::new(env!("CARGO_BIN_EXE_golang-symbol"))
        .arg(fixture("app.stripped"))
        .env("PATH", "")
        .env("GOLANG_SYMBOL_EXECUTION_SENTINEL", &sentinel)
        .output()
        .unwrap();
    assert!(!sentinel.exists());
    assert!(serde_json::from_slice::<serde_json::Value>(&out.stdout).is_ok());
}

#[test]
fn fixture_identity_matches_go_version_m() {
    let path = fixture("app.stripped");
    let oracle = Command::new("go")
        .args(["version", "-m"])
        .arg(&path)
        .env("GOTOOLCHAIN", "local")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .output()
        .unwrap();
    assert!(oracle.status.success());
    let text = String::from_utf8(oracle.stdout).unwrap();
    let (_, j) = run_json(&path);
    let version = text.lines().next().unwrap().rsplit_once(": ").unwrap().1;
    assert_eq!(j["artifact"]["go_version"], version);
    assert!(text.contains("dep\texample.com/golang-symbol/fixturedep\tv1.2.3"));
    assert!(text.contains("=>\t../dependency"));
    assert_eq!(
        j["build"]["main"]["path"],
        "example.com/golang-symbol/fixture"
    );
    assert_eq!(j["build"]["main"]["version"], "(devel)");
}
