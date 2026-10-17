//! Drives the `logring` binary end to end on an image `make-image` built, like a user would from a shell: the
//! CLI's own "done when" (P2628#16oct-1001h) alongside `logring_export`'s crate-level tests.

use assert_cmd::Command;
use predicates::prelude::*;

fn logring() -> Command {
    Command::cargo_bin("logring").unwrap()
}

#[test]
fn make_image_then_info_export_cat_check() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("demo.wlr");

    logring()
        .args([
            "make-image",
            image.to_str().unwrap(),
            "--tracks",
            "2",
            "--records",
            "20",
        ])
        .assert()
        .success();
    assert!(image.exists());

    logring()
        .args(["info", image.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("sensor_0"))
        .stdout(predicate::str::contains("resolved"));

    let csv = dir.path().join("sensor_0.csv");
    logring()
        .args([
            "export",
            image.to_str().unwrap(),
            "--track",
            "sensor_0",
            "--format",
            "csv",
            "-o",
            csv.to_str().unwrap(),
        ])
        .assert()
        .success();
    let csv_text = std::fs::read_to_string(&csv).unwrap();
    assert!(
        csv_text.lines().count() > 1,
        "a header and at least one record"
    );

    let mcap = dir.path().join("demo.mcap");
    logring()
        .args([
            "export",
            image.to_str().unwrap(),
            "--format",
            "mcap",
            "-o",
            mcap.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(mcap.metadata().unwrap().len() > 0);

    let mf4 = dir.path().join("sensor_1.mf4");
    logring()
        .args([
            "export",
            image.to_str().unwrap(),
            "--track",
            "sensor_1",
            "--format",
            "mf4",
            "-o",
            mf4.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(mf4.metadata().unwrap().len() > 0);

    logring()
        .args(["cat", image.to_str().unwrap(), "--track", "sensor_0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pos\":0"));

    logring()
        .args(["check", image.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("clean"));
}

#[test]
fn check_reports_an_unknown_track() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("demo.wlr");
    logring()
        .args(["make-image", image.to_str().unwrap()])
        .assert()
        .success();

    logring()
        .args(["cat", image.to_str().unwrap(), "--track", "no-such-track"])
        .assert()
        .failure();
}

#[test]
fn version_has_a_git_sha() {
    logring()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"logring \d+\.\d+\.\d+ \([0-9a-f]{7,}").unwrap());
}
