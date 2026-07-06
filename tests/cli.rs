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
    // standalone `app` (the workspace member, excluded) with two non-member path
    // dependencies: `dep` (MIT, ships LICENSE + NOTICE) and `dep2` (MIT, authors only).
    let dir = std::env::temp_dir().join(format!("tribute-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n\
         authors = [\"Dep Author <dep@example.com>\"]\n",
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(&dir.join("dep/LICENSE"), "Copyright (c) 2024 Dep Author\n\nPermission is hereby granted...\n");
    write(&dir.join("dep/NOTICE"), "dep\nCopyright 2024 Dep Author\n");
    write(
        &dir.join("dep2/Cargo.toml"),
        "[package]\nname = \"dep2\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n\
         authors = [\"Alice <alice@example.com>\"]\n",
    );
    write(&dir.join("dep2/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep = { path = \"../dep\" }\ndep2 = { path = \"../dep2\" }\n",
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

    // dep's copyright line comes from its LICENSE; dep2 has none, so its
    // metadata authors show instead (email stripped).
    assert!(manifest.contains("Copyright (c) 2024 Dep Author"), "manifest:\n{manifest}");
    assert!(manifest.contains("by Alice"), "manifest:\n{manifest}");
    assert!(!manifest.contains("alice@example.com"), "author email must be stripped:\n{manifest}");

    // dep's NOTICE is bundled and linked from the manifest.
    assert!(manifest.contains("NOTICES/dep-1.0.0.txt"), "manifest:\n{manifest}");
    let notice = fs::read_to_string(dir.join("app/NOTICES/dep-1.0.0.txt")).unwrap();
    assert!(notice.contains("Copyright 2024 Dep Author"), "notice:\n{notice}");

    // --check passes immediately after a write.
    assert!(tribute(&manifest_path, &["--check"]).status.success(), "check should pass after write");

    // tampering a bundled notice makes --check fail; a rewrite repairs it.
    fs::write(dir.join("app/NOTICES/dep-1.0.0.txt"), "stale\n").unwrap();
    assert!(!tribute(&manifest_path, &["--check"]).status.success(), "check should fail on stale notice");
    assert!(tribute(&manifest_path, &[]).status.success());
    assert!(tribute(&manifest_path, &["--check"]).status.success(), "rewrite should repair the notice");

    // tampering the manifest makes --check fail.
    fs::write(dir.join("app/THIRD-PARTY.md"), "stale\n").unwrap();
    assert!(!tribute(&manifest_path, &["--check"]).status.success(), "check should fail on stale output");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn features_flag_attributes_optional_deps() {
    // `dep` is pulled in only by the optional `extra` feature. a default run must not
    // attribute it; forwarding `--features extra` to cargo metadata must.
    let dir = std::env::temp_dir().join(format!("tribute-feat-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep = { path = \"../dep\", optional = true }\n\n[features]\nextra = [\"dep\"]\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    let manifest_path = dir.join("app/Cargo.toml");

    let out = tribute(&manifest_path, &[]);
    assert!(out.status.success(), "default run failed: {}", String::from_utf8_lossy(&out.stderr));
    let default = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(!default.contains("dep 1.0.0"), "optional dep must be omitted by default:\n{default}");

    let out = tribute(&manifest_path, &["--features", "extra"]);
    assert!(out.status.success(), "--features run failed: {}", String::from_utf8_lossy(&out.stderr));
    let enabled = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(enabled.contains("dep 1.0.0"), "--features extra must attribute the optional dep:\n{enabled}");

    fs::remove_dir_all(&dir).ok();
}
