// End-to-end test of the whole flow (dep-closure walk, license choice, render, write,
// --check) against a real `cargo metadata`. The fixture uses only a path dependency, so
// it resolves offline and deterministically with no crates.io access.

use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tribute(manifest: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-tribute")).arg("--manifest-path").arg(manifest).args(extra).output().unwrap()
}

#[test]
fn writes_manifest_then_check_roundtrips() {
    // standalone `app` (the workspace member, excluded) with a non-member path
    // dependency `dep` (MIT, attributed).
    let dir = std::env::temp_dir().join(format!("tribute-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = { path = \"../dep\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    let manifest_path = dir.join("app/Cargo.toml");

    // write: `dep` attributed as MIT, workspace member `app` excluded.
    let out = tribute(&manifest_path, &[]);
    assert!(out.status.success(), "write failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("## MIT"), "manifest:\n{manifest}");
    assert!(manifest.contains("dep 1.0.0"), "manifest:\n{manifest}");
    assert!(!manifest.contains("app 0.0.0"), "workspace member must be excluded:\n{manifest}");
    assert!(dir.join("app/LICENSES/MIT.txt").exists());

    // --check passes immediately after a write.
    assert!(tribute(&manifest_path, &["--check"]).status.success(), "check should pass after write");

    // tampering the manifest makes --check fail.
    fs::write(dir.join("app/THIRD-PARTY.md"), "stale\n").unwrap();
    assert!(!tribute(&manifest_path, &["--check"]).status.success(), "check should fail on stale output");

    fs::remove_dir_all(&dir).ok();
}
