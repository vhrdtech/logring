//! Drives the `logring` binary end to end on an image `make-image` built, like a user would from a shell: the
//! CLI's own "done when" alongside `logring_export`'s crate-level tests. [reading_md_quotes_a_real_run] pins
//! "Reading a log on a PC"'s CLI transcripts to a real run on the drive example's own card, so a change to its
//! output fails here until the page says the new one (the same discipline as `logring_figures`' `numbers.rs`,
//! for CLI text instead of numbers in prose).

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

fn reading_md() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reading.md");
    std::fs::read_to_string(&path).unwrap()
}

fn quotes(page: &str, line: &str) {
    assert!(page.contains(line), "docs/reading.md does not say {line:?}");
}

/// "Reading a log on a PC" walks the drive's own card: every CLI block it quotes is a real run of
/// `logring_example::story::minute_card()` through the real binary, checked here line by line.
#[test]
fn reading_md_quotes_a_real_run_on_the_drives_card() {
    let page = reading_md();
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("drive.wlr");
    std::fs::write(&image, logring_example::story::minute_card()).unwrap();

    let info = logring()
        .args(["info", image.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(info.status.success());
    let info = String::from_utf8(info.stdout).unwrap();
    for line in [
        "17 data segments, 1 meta segments, 1 sessions, 1 bundles",
        "session 1: drive-0042 (drive fw 1.4.0 drive rev C), bundle 1eb8ced7c6bdad9e",
        "bundle 1eb8ced7c6bdad9e: 797 bytes, resolved",
        "log                      key 3aa950c141a0a925  path [0]  31 record(s), 0 decode error(s)  [resolved]",
        "can0                     key 7a9ea8d1ee0c2247  path [2]  60000 record(s), 0 decode error(s)  [resolved]",
        "mode                     key 9aaea4b8eb6aec94  path [1]  3 record(s), 0 decode error(s)  [resolved]",
        "vbat                     key daa3fcc997d665b6  path [3]  60 record(s), 0 decode error(s)  [resolved]",
        "0 marker(s), 1 recording(s) started, 1 stopped, 0 drop event(s), 0 card event(s)",
    ] {
        assert!(
            info.contains(line),
            "`logring info` does not print {line:?}"
        );
        quotes(&page, line);
    }

    let export = logring()
        .current_dir(dir.path())
        .args([
            "export",
            image.to_str().unwrap(),
            "--track",
            "can0",
            "--format",
            "mf4",
            "-o",
            "can0.mf4",
        ])
        .output()
        .unwrap();
    assert!(export.status.success());
    let export = String::from_utf8(export.stdout).unwrap();
    let line = "can0 -> can0.mf4";
    assert!(
        export.contains(line),
        "`logring export` does not print {line:?}"
    );
    quotes(&page, line);
    assert!(
        dir.path().join("can0.mf4").metadata().unwrap().len() > 0,
        "can0 exports to MF4"
    );

    let cat = logring()
        .args(["cat", image.to_str().unwrap(), "--track", "can0"])
        .output()
        .unwrap();
    assert!(cat.status.success());
    let cat = String::from_utf8(cat.stdout).unwrap();
    for line in [
        r#"{"pos":0,"t_ps":0,"value":{"data":[184,11,0,0,0,0,0,0],"id":257}}"#,
        r#"{"pos":1,"t_ps":1000000000,"value":{"data":[185,11,1,0,0,0,0,0],"id":257}}"#,
    ] {
        assert!(
            cat.lines().any(|l| l == line),
            "`logring cat` does not print {line:?}"
        );
        quotes(&page, line);
    }

    let log = logring_export::open(&image).unwrap();
    let recs = logring_export::events::recordings(&log);
    let rec = recs.first().expect("one recording");
    assert_eq!((rec.start_pos, rec.stop_pos), (1, Some(2)));
    quotes(&page, "1 and 2 for this drive's");

    let bundles = logring_export::parse_bundles(&log);
    let tracks = logring_export::decode::tracks(&log, &bundles);
    let can0 = tracks.iter().find(|t| t.name == "can0").unwrap();
    assert_eq!(can0.items.len(), 60000);
    let (start, stop) = (rec.start_t_ps.unwrap(), rec.stop_t_ps.unwrap());
    let in_range = can0
        .items
        .iter()
        .filter(|it| {
            it.head
                .t
                .as_ref()
                .and_then(logring::reader::time_ps)
                .is_some_and(|ps| ps >= start && ps <= stop)
        })
        .count();
    assert_eq!(in_range, 59000);
    quotes(&page, "holds 60 000 records of its own");
    quotes(&page, "`during` holds 59 000 of `can0`'s 60 000 rows");

    let bundleless = write_bundle_less_ring(dir.path());
    let check = logring()
        .args(["check", bundleless.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(check.status.success());
    let check = String::from_utf8(check.stdout).unwrap();
    let line =
        "warning track key.a8231e12f9543fab has no resolved type, 3 record(s) left undecoded";
    assert!(
        check.contains(line),
        "`logring check` does not print {line:?}"
    );
    assert!(check.contains("clean"));
    quotes(&page, line);
}
