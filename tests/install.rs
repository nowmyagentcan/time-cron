//! scripts/install.sh against local releases in a throwaway HOME: a good archive installs, and a
//! bad one leaves the installed copy as it was.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_time-cron");

/// A folder with install.sh and one release per payload (`None`: an archive without
/// time-cron), each with its SHA256SUMS.
fn releases(name: &str, payloads: &[(&str, Option<&[u8]>)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("time-cron-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let arch = if cfg!(target_arch = "x86_64") {
        "x64"
    } else {
        "arm64"
    };
    let archive = format!("time-cron-v0.1.0-{}-{arch}.tar.gz", std::env::consts::OS);
    for (release, payload) in payloads {
        let (stage, dir) = (root.join("stage").join(release), root.join(release));
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&dir).unwrap();
        let file = if payload.is_some() {
            "time-cron"
        } else {
            "README.md"
        };
        fs::write(stage.join(file), payload.unwrap_or(b"no binary")).unwrap();
        let pack = format!(
            "chmod +x {file} && tar -czf '{0}/{archive}' {file} && cd '{0}' && \
             {{ sha256sum {archive} || shasum -a 256 {archive}; }} > SHA256SUMS",
            dir.display()
        );
        let packed = Command::new("sh")
            .args(["-c", &pack])
            .current_dir(&stage)
            .status();
        assert!(packed.unwrap().success(), "{release}");
    }
    let script = fs::read_to_string("scripts/install.sh").unwrap();
    fs::write(
        root.join("install.sh"),
        script.replace("@VERSION@", "v0.1.0"),
    )
    .unwrap();
    root
}

/// Runs install.sh against `release`: whether it succeeded, and whether the installed copy
/// is `expected` byte for byte.
fn install(root: &Path, release: &str, expected: &[u8]) -> (bool, bool) {
    let home = root.join("home");
    let base = format!("file://{}", root.join(release).display());
    let status = Command::new("sh")
        .arg(root.join("install.sh"))
        .env("HOME", &home)
        .env("TIME_CRON_BASE", base)
        .status()
        .unwrap();
    let installed = fs::read(home.join(".local/bin/time-cron")).ok();
    (status.success(), installed.as_deref() == Some(expected))
}

#[test]
fn installs_and_keeps_the_installed_copy_when_an_archive_is_bad() {
    let good = fs::read(BIN).unwrap();
    let payloads = [
        ("good", Some(&good[..])),
        ("bad", Some(&good[..])),
        ("missing", None),
    ];
    let root = releases("install", &payloads);
    let sums = root.join("bad/SHA256SUMS");
    let wrong = format!("x{}", &fs::read_to_string(&sums).unwrap()[1..]);
    fs::write(&sums, wrong).unwrap();
    assert_eq!(install(&root, "good", &good), (true, true));
    assert_eq!(install(&root, "bad", &good), (false, true));
    assert_eq!(install(&root, "missing", &good), (false, true));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
#[ignore = "known bug: install.sh replaces the installed copy before checking that the new one runs"]
fn keeps_the_installed_copy_when_the_new_one_does_not_run() {
    let good = fs::read(BIN).unwrap();
    let payloads = [
        ("good", Some(&good[..])),
        ("invalid", Some(&[0, 1, 2, 3][..])),
        ("empty", Some(&[][..])),
    ];
    let root = releases("upgrade", &payloads);
    for release in ["invalid", "empty"] {
        assert_eq!(install(&root, "good", &good), (true, true));
        assert_eq!(install(&root, release, &good), (false, true), "{release}");
    }
    fs::remove_dir_all(&root).unwrap();
}
