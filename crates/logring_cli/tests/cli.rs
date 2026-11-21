//! Drives the `logring` binary end to end on an image `make-image` built, like a user would from a shell: the
//! CLI's own "done when" alongside `logring_export`'s crate-level tests.

use assert_cmd::Command;
use logring::format::header::{Durability, TrackDesc};
use logring::tests_support::block_on;
use logring::writer::{Buffers, Config, Logring, SessionInfo};
use logring::{FullPolicy, UNib32};
use predicates::prelude::*;
use wire_weaver::ww_version::{FullVersion, Version};
use ww_stream::store::log::SimSd;

fn logring() -> Command {
    Command::cargo_bin("logring").unwrap()
}

/// A ring with one track and a few records, but no API bundle: a device whose firmware never set
/// `SessionInfo::bundle`, or an old session logged under an API that was later dropped. Nothing about it is
/// damaged; its track simply can't be resolved to a type.
fn write_bundle_less_ring(dir: &std::path::Path) -> std::path::PathBuf {
    const BUF: usize = 1024;
    const TRACKS: usize = 4;
    const IMAGE: usize = 4096 * 8;

    let path = [UNib32(1)];
    let tracks = [TrackDesc {
        signature: None,
        global_id: None,
        durability: Durability::Normal,
        rate: None,
        ..TrackDesc::new(&path)
    }];
    let info = SessionInfo {
        api: FullVersion::new("test_fw", Version::new(1, 0, 0)),
        device_id: "test",
        fw: "test_fw 1.0.0",
        hw: "none",
        tracks: &tracks,
        bundle: None,
    };
    let cfg = Config {
        segment_size: 4096,
        meta_segments: 2,
        flush_bytes: 1024,
        flush_interval_ms: 1000,
        full: FullPolicy::DropOldest,
        seed: 1,
        ..Config::default()
    };
    let medium = SimSd::from_image(vec![0u8; IMAGE], 1);
    let mut bufs = Box::new(Buffers::<BUF, TRACKS>::new());
    let mut lr = Logring::new(medium, &mut bufs, cfg, info).unwrap();
    block_on(lr.mount()).unwrap();
    for v in [10u8, 20, 30] {
        block_on(lr.push(1, &[v, 0, 0, 0], None)).unwrap();
    }
    block_on(lr.sync()).unwrap();
    let image = lr.into_medium().image();

    let out = dir.join("bundleless.wlr");
    std::fs::write(&out, &image).unwrap();
    out
}

/// A card with the drive's own bundle (so `can0` resolves), but one `can0` item that doesn't decode against
/// `CanFrame`: a torn write or real corruption, not a missing bundle. `check` must still fail on it, and `info`
/// must still call it a decode error — the thing this commit's fix must not break.
fn write_card_with_a_bad_can0_record(dir: &std::path::Path) -> std::path::PathBuf {
    let mut bytes = vec![0u8; logring_example::card::SIM_CARD_BYTES];
    let mut bufs: Buffers<{ logring_example::drive::BUF }, { logring_example::drive::TRACKS }> =
        Buffers::new();
    block_on(async {
        let mut lr = Logring::new(
            logring_example::SimCard::new(&mut bytes),
            &mut bufs,
            logring_example::drive::config(),
            logring_example::drive::session(),
        )
        .unwrap();
        lr.mount().await.unwrap();
        lr.push(logring_example::drive::track::CAN0, &[0xffu8; 11], None)
            .await
            .unwrap();
        lr.sync().await.unwrap();
    });

    let out = dir.join("bad_can0.wlr");
    std::fs::write(&out, &bytes).unwrap();
    out
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
fn check_is_healthy_on_a_card_without_a_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let image = write_bundle_less_ring(dir.path());

    logring()
        .args(["check", image.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning"))
        .stdout(predicate::str::contains("clean"))
        .stdout(predicate::str::contains("decode error").not());

    logring()
        .args(["info", image.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("3 record(s), type unresolved"))
        .stdout(predicate::str::contains("decode error").not());
}

/// A resolved track's own decode error (real corruption) must still fail `check` and still show up as a decode
/// error in `info` — unlike an unresolved track's records, which `check_is_healthy_on_a_card_without_a_bundle`
/// shows do not.
#[test]
fn check_fails_on_a_resolved_tracks_decode_error() {
    let dir = tempfile::tempdir().unwrap();
    let image = write_card_with_a_bad_can0_record(dir.path());

    logring()
        .args(["check", image.to_str().unwrap()])
        .assert()
        .failure()
        .stdout(predicate::str::contains("can0: 1 record(s) didn't decode"));

    logring()
        .args(["info", image.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 decode error(s)  [resolved]"));
}

#[test]
fn version_has_a_git_sha() {
    logring()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"logring \d+\.\d+\.\d+ \([0-9a-f]{7,}").unwrap());
}
