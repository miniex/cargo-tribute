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
fn exception_allows_a_license_for_one_crate() {
    // `dep` is MPL-2.0, which is not in the default accepted set: a plain run must
    // fail, and a [[exception]] entry for that crate must let it through.
    let dir = std::env::temp_dir().join(format!("tribute-exc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MPL-2.0\"\n",
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = { path = \"../dep\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    let manifest_path = dir.join("app/Cargo.toml");

    let out = tribute(&manifest_path, &[]);
    assert!(!out.status.success(), "MPL-2.0 must fail without an exception");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not in the accepted set"));

    write(&dir.join("app/tribute.toml"), "[[exception]]\nname = \"dep\"\nallow = [\"MPL-2.0\"]\n");
    let out = tribute(&manifest_path, &[]);
    assert!(out.status.success(), "exception run failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("## MPL-2.0"), "manifest:\n{manifest}");
    assert!(dir.join("app/LICENSES/MPL-2.0.txt").exists());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn include_dev_and_build_deps_opt_in() {
    // `devdep`/`builddep` are dev- and build-dependencies: skipped by default,
    // attributed once tribute.toml opts in.
    let dir = std::env::temp_dir().join(format!("tribute-kinds-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for name in ["devdep", "builddep"] {
        write(
            &dir.join(format!("{name}/Cargo.toml")),
            &format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n"),
        );
        write(&dir.join(format!("{name}/src/lib.rs")), "");
    }
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [dev-dependencies]\ndevdep = { path = \"../devdep\" }\n\n\
         [build-dependencies]\nbuilddep = { path = \"../builddep\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    write(&dir.join("app/build.rs"), "fn main() {}\n");
    let manifest_path = dir.join("app/Cargo.toml");

    let out = tribute(&manifest_path, &[]);
    assert!(out.status.success(), "default run failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(!manifest.contains("devdep"), "dev dep must be skipped by default:\n{manifest}");
    assert!(!manifest.contains("builddep"), "build dep must be skipped by default:\n{manifest}");

    write(&dir.join("app/tribute.toml"), "include-dev = true\ninclude-build = true\n");
    let out = tribute(&manifest_path, &[]);
    assert!(out.status.success(), "opt-in run failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("devdep 1.0.0"), "include-dev must attribute the dev dep:\n{manifest}");
    assert!(manifest.contains("builddep 1.0.0"), "include-build must attribute the build dep:\n{manifest}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn extra_and_licenseref_are_attributed() {
    // `dep` declares a LicenseRef license whose text comes from [[license-text]];
    // an [[extra]] entry attributes vendored non-crate code under Zlib.
    let dir = std::env::temp_dir().join(format!("tribute-extra-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"LicenseRef-weird\"\n",
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = { path = \"../dep\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    write(&dir.join("app/weird-license.txt"), "the weird license text\n");
    write(
        &dir.join("app/tribute.toml"),
        "accepted = [\"MIT\", \"Zlib\", \"LicenseRef-weird\"]\n\n\
         [[extra]]\nname = \"zlib (vendored)\"\nexpression = \"Zlib\"\nurl = \"https://zlib.net\"\n\
         copyright = \"Copyright (C) 1995 Jean-loup Gailly\"\n\n\
         [[license-text]]\nid = \"LicenseRef-weird\"\nfile = \"weird-license.txt\"\n",
    );
    let manifest_path = dir.join("app/Cargo.toml");

    let out = tribute(&manifest_path, &[]);
    assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("## LicenseRef-weird"), "manifest:\n{manifest}");
    assert!(manifest.contains("dep 1.0.0"), "manifest:\n{manifest}");
    assert!(manifest.contains("## Zlib"), "manifest:\n{manifest}");
    assert!(manifest.contains("[zlib (vendored)](https://zlib.net)"), "manifest:\n{manifest}");
    assert!(manifest.contains("Copyright (C) 1995 Jean-loup Gailly"), "manifest:\n{manifest}");
    let text = fs::read_to_string(dir.join("app/LICENSES/LicenseRef-weird.txt")).unwrap();
    assert_eq!(text, "the weird license text\n");
    assert!(dir.join("app/LICENSES/Zlib.txt").exists());

    // MIT is accepted but unreferenced -> warns; Zlib counts as used via the
    // [[extra]], and a LicenseRef never triggers the unknown-SPDX-id warning.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("accepted license 'MIT' matched no dependency"), "stderr:\n{stderr}");
    assert!(!stderr.contains("'Zlib' matched no dependency"), "extra must count as use:\n{stderr}");
    assert!(!stderr.contains("LicenseRef-weird' matched no dependency"), "stderr:\n{stderr}");
    assert!(!stderr.contains("is not a known SPDX id"), "LicenseRef must not warn:\n{stderr}");

    // --check roundtrips; tampering the copied LicenseRef text makes it fail.
    assert!(tribute(&manifest_path, &["--check"]).status.success(), "check should pass after write");
    fs::write(dir.join("app/LICENSES/LicenseRef-weird.txt"), "stale\n").unwrap();
    assert!(!tribute(&manifest_path, &["--check"]).status.success(), "check should fail on stale text");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn unused_accepted_entry_warns() {
    // an explicit accepted list with an entry no dependency references warns; the
    // used entry does not.
    let dir = std::env::temp_dir().join(format!("tribute-unused-{}", std::process::id()));
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
    write(&dir.join("app/tribute.toml"), "accepted = [\"MIT\", \"Zlib\"]\n");

    let out = tribute(&dir.join("app/Cargo.toml"), &[]);
    assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("accepted license 'Zlib' matched no dependency"), "stderr:\n{stderr}");
    assert!(!stderr.contains("'MIT' matched"), "used entry must not warn:\n{stderr}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn format_text_and_cyclonedx_report_without_writing() {
    // one fixture, both stdout formats: `dep` is MIT with a LICENSE and a NOTICE.
    let dir = std::env::temp_dir().join(format!("tribute-fmt-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(&dir.join("dep/LICENSE"), "Copyright (c) 2024 Dep Author\n");
    write(&dir.join("dep/NOTICE"), "dep notice body\n");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = { path = \"../dep\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    let manifest_path = dir.join("app/Cargo.toml");

    // --format text: one flat document with the entry, the license text, and the NOTICE.
    let out = tribute(&manifest_path, &["--format", "text"]);
    assert!(out.status.success(), "text failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Third-party licenses"), "text:\n{text}");
    assert!(text.contains("- dep 1.0.0"), "text:\n{text}");
    assert!(text.contains("Copyright (c) 2024 Dep Author"), "text:\n{text}");
    assert!(text.contains("Permission is hereby granted"), "MIT body must be included:\n{text}");
    assert!(text.contains("NOTICE for dep 1.0.0"), "text:\n{text}");
    assert!(text.contains("dep notice body"), "text:\n{text}");

    // --format cyclonedx: valid JSON carrying the license text and copyright.
    let out = tribute(&manifest_path, &["--format", "cyclonedx"]);
    assert!(out.status.success(), "cyclonedx failed: {}", String::from_utf8_lossy(&out.stderr));
    let bom: serde_json::Value = serde_json::from_slice(&out.stdout).expect("cyclonedx output must be valid JSON");
    assert_eq!(bom["bomFormat"], "CycloneDX");
    assert_eq!(bom["specVersion"], "1.6");
    let comp = &bom["components"][0];
    assert_eq!(comp["name"], "dep");
    assert_eq!(comp["version"], "1.0.0");
    assert!(comp["purl"].is_null(), "a path dep must carry no purl");
    assert_eq!(comp["licenses"][0]["license"]["id"], "MIT");
    let body = comp["licenses"][0]["license"]["text"]["content"].as_str().unwrap();
    assert!(body.contains("Permission is hereby granted"), "license text must be embedded");
    assert_eq!(comp["copyright"], "Copyright (c) 2024 Dep Author");
    assert_eq!(comp["properties"][0]["value"], "MIT");

    // neither report mode writes anything.
    assert!(!dir.join("app/THIRD-PARTY.md").exists());
    assert!(!dir.join("app/LICENSES").exists());

    // an unknown format is rejected up front.
    assert!(!tribute(&manifest_path, &["--format", "bogus"]).status.success());

    fs::remove_dir_all(&dir).ok();
}

// a plain app fixture with one MIT path dep; several tests below share this shape.
fn write_app(dir: &Path, dep_license: &str) -> std::path::PathBuf {
    write(
        &dir.join("dep/Cargo.toml"),
        &format!("[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"{dep_license}\"\n"),
    );
    write(&dir.join("dep/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = { path = \"../dep\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    dir.join("app/Cargo.toml")
}

#[test]
fn package_selection_limits_the_closure() {
    // a two-member workspace: -p a must attribute only a's dependency.
    let dir = std::env::temp_dir().join(format!("tribute-pkg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    // exclude the deps, or cargo absorbs the in-tree path deps as members and the
    // members-are-excluded rule would leave nothing to attribute.
    write(
        &dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nexclude = [\"depa\", \"depb\"]\nresolver = \"2\"\n",
    );
    for (member, dep) in [("a", "depa"), ("b", "depb")] {
        write(
            &dir.join(format!("{dep}/Cargo.toml")),
            &format!("[package]\nname = \"{dep}\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n"),
        );
        write(&dir.join(format!("{dep}/src/lib.rs")), "");
        write(
            &dir.join(format!("{member}/Cargo.toml")),
            &format!(
                "[package]\nname = \"{member}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
                 [dependencies]\n{dep} = {{ path = \"../{dep}\" }}\n"
            ),
        );
        write(&dir.join(format!("{member}/src/lib.rs")), "");
    }
    let manifest_path = dir.join("Cargo.toml");

    let out = tribute(&manifest_path, &["-p", "a"]);
    assert!(out.status.success(), "-p a failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("depa"), "manifest:\n{manifest}");
    assert!(!manifest.contains("depb"), "-p a must exclude b's deps:\n{manifest}");

    // an unknown member is an error, not a silently empty run.
    assert!(!tribute(&manifest_path, &["-p", "nope"]).status.success());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn from_deny_reuses_the_allowlist() {
    // dep is MPL-2.0: rejected by the defaults, allowed by deny.toml's exceptions.
    let dir = std::env::temp_dir().join(format!("tribute-deny-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let manifest_path = write_app(&dir, "MPL-2.0");
    write(
        &dir.join("app/deny.toml"),
        "[licenses]\nallow = [\"MIT\", \"Apache-2.0\"]\nexceptions = [{ allow = [\"MPL-2.0\"], crate = \"dep\" }]\n",
    );
    let deny = dir.join("app/deny.toml");
    let deny = deny.to_str().unwrap();

    let out = tribute(&manifest_path, &["--from-deny", deny]);
    assert!(out.status.success(), "--from-deny failed: {}", String::from_utf8_lossy(&out.stderr));
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("## MPL-2.0"), "manifest:\n{manifest}");

    // an explicit accepted list in tribute.toml conflicts with --from-deny.
    write(&dir.join("app/tribute.toml"), "accepted = [\"MIT\"]\n");
    let out = tribute(&manifest_path, &["--from-deny", deny]);
    assert!(!out.status.success(), "conflicting sources must fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("keep one source"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn exit_codes_distinguish_failure_kinds() {
    let dir = std::env::temp_dir().join(format!("tribute-exit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let manifest_path = write_app(&dir, "GPL-3.0-only");

    // 1: the license policy failed.
    assert_eq!(tribute(&manifest_path, &[]).status.code(), Some(1));

    // 2: --check found stale output.
    write(
        &dir.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    );
    assert_eq!(tribute(&manifest_path, &["--check"]).status.code(), Some(2));

    // 3: anything else (unreadable config).
    write(&dir.join("app/tribute.toml"), "accepted = 3\n");
    assert_eq!(tribute(&manifest_path, &[]).status.code(), Some(3));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn quiet_suppresses_the_summary() {
    let dir = std::env::temp_dir().join(format!("tribute-quiet-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let manifest_path = write_app(&dir, "MIT");

    let out = tribute(&manifest_path, &["--quiet"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "quiet must print nothing on success");
    // the files are still written.
    assert!(dir.join("app/THIRD-PARTY.md").exists());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn init_scaffolds_a_config() {
    let dir = std::env::temp_dir().join(format!("tribute-init-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let manifest_path = write_app(&dir, "MIT");

    let out = tribute(&manifest_path, &["init"]);
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let cfg = fs::read_to_string(dir.join("app/tribute.toml")).unwrap();
    assert!(cfg.contains("# accepted = ["), "template must show the defaults:\n{cfg}");
    // everything is commented out: the scaffold must not change behavior.
    assert!(tribute(&manifest_path, &[]).status.success());

    // a second init must not clobber the existing config.
    assert!(!tribute(&manifest_path, &["init"]).status.success());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn skip_private_and_proc_macros_opt_out() {
    // `pm` is a proc-macro pulling `pmdep`; `dep` is a plain path dep. the skip
    // options drop them (and the proc-macro's subtree) from the attribution.
    let dir = std::env::temp_dir().join(format!("tribute-skip-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for name in ["dep", "pmdep"] {
        write(
            &dir.join(format!("{name}/Cargo.toml")),
            &format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n"),
        );
        write(&dir.join(format!("{name}/src/lib.rs")), "");
    }
    write(
        &dir.join("pm/Cargo.toml"),
        "[package]\nname = \"pm\"\nversion = \"1.0.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n\n\
         [lib]\nproc-macro = true\n\n[dependencies]\npmdep = { path = \"../pmdep\" }\n",
    );
    write(&dir.join("pm/src/lib.rs"), "");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep = { path = \"../dep\" }\npm = { path = \"../pm\" }\n",
    );
    write(&dir.join("app/src/main.rs"), "fn main() {}\n");
    let manifest_path = dir.join("app/Cargo.toml");

    // default: everything is attributed.
    assert!(tribute(&manifest_path, &[]).status.success());
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    for name in ["dep", "pm 1.0.0", "pmdep"] {
        assert!(manifest.contains(name), "missing {name}:\n{manifest}");
    }

    // skip-proc-macros drops pm and its subtree; dep stays.
    write(&dir.join("app/tribute.toml"), "skip-proc-macros = true\n");
    assert!(tribute(&manifest_path, &[]).status.success());
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(manifest.contains("dep 1.0.0"), "manifest:\n{manifest}");
    assert!(!manifest.contains("pm 1.0.0"), "manifest:\n{manifest}");
    assert!(!manifest.contains("pmdep"), "the proc-macro subtree must go too:\n{manifest}");

    // skip-private drops the path deps entirely (none are from crates.io).
    write(&dir.join("app/tribute.toml"), "skip-private = true\n");
    assert!(tribute(&manifest_path, &[]).status.success());
    let manifest = fs::read_to_string(dir.join("app/THIRD-PARTY.md")).unwrap();
    assert!(!manifest.contains("dep 1.0.0"), "manifest:\n{manifest}");

    fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "audit")]
#[test]
fn audit_flags_a_mismatched_license_file() {
    // dep declares Zlib but ships the MIT text: --audit must call it out, exit 0.
    let dir = std::env::temp_dir().join(format!("tribute-audit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let manifest_path = write_app(&dir, "Zlib");
    write(
        &dir.join("dep/LICENSE"),
        "MIT License\n\nCopyright (c) 2024 Dep Author\n\nPermission is hereby granted, free of charge, to any \
         person obtaining a copy of this software and associated documentation files (the \"Software\"), to deal \
         in the Software without restriction, including without limitation the rights to use, copy, modify, \
         merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to \
         whom the Software is furnished to do so, subject to the following conditions:\n\nThe above copyright \
         notice and this permission notice shall be included in all copies or substantial portions of the \
         Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, \
         INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND \
         NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR \
         OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN \
         CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n",
    );

    let out = tribute(&manifest_path, &["--audit"]);
    assert!(out.status.success(), "audit is advisory and must exit 0");
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(report.contains("matches MIT"), "report:\n{report}");
    assert!(report.contains("declared license is 'Zlib'"), "report:\n{report}");

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
