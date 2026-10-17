//! The `logring` subcommands: `info`, `export`, `cat`, `check`, `make-image`.

use crate::cli::Format;
use crate::color;
use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use logring::system::SystemEvent;
use logring_export::decode::{self, Track};
use logring_export::{self, LogFile};
use logring_export::{Bundle, csv_export, mcap_export, mf4_export};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn load(path: &Path) -> Result<(LogFile, BTreeMap<u64, Bundle>)> {
    let log = logring_export::open(path).with_context(|| format!("opening {}", path.display()))?;
    let bundles = logring_export::parse_bundles(&log);
    Ok((log, bundles))
}

fn find_track<'a, 'b>(tracks: &'a [Track<'b>], spec: &str) -> Result<&'a Track<'b>> {
    if let Some(t) = tracks.iter().find(|t| t.name == spec) {
        return Ok(t);
    }
    let hex = spec.strip_prefix("key.").unwrap_or(spec);
    if let Ok(key) = u64::from_str_radix(hex, 16)
        && let Some(t) = tracks.iter().find(|t| t.info.key == key)
    {
        return Ok(t);
    }
    bail!("no track named or keyed {spec:?} (see `logring info`)");
}

pub fn info(path: &Path) -> Result<()> {
    let (log, bundles) = load(path)?;
    let tracks = decode::tracks(&log, &bundles);
    let events: Vec<SystemEvent<'_>> = decode::system_events(&log);

    println!("{}", color::bold(&format!("{}", path.display())));
    println!(
        "  {} data segments, {} meta segments, {} sessions, {} bundles",
        log.scan.data.len(),
        log.scan.meta.len(),
        log.sessions.len(),
        log.bundles.len()
    );
    for s in &log.sessions {
        println!(
            "  session {}: {} ({} {}), bundle {:016x}",
            s.session, s.device_id, s.fw, s.hw, s.bundle_hash
        );
    }
    for (hash, b) in &log.bundles {
        let parsed = bundles.get(hash);
        let status = match parsed {
            Some(b) if b.unresolved == 0 => color::good("resolved"),
            Some(b) => color::dim(&format!(
                "{} type(s)/trait(s) left unresolved",
                b.unresolved
            )),
            None => color::bad("failed to decode"),
        };
        println!("  bundle {hash:016x}: {} bytes, {status}", b.bytes.len());
    }
    println!("{}", color::bold("tracks:"));
    for t in &tracks {
        let kind = match t.ty {
            Some(_) => color::good("resolved"),
            None => color::bad("unresolved"),
        };
        println!(
            "  {:<24} key {:016x}  path {:?}  {} records, {} decode error(s)  [{kind}]",
            t.name,
            t.info.key,
            t.info.path,
            t.items.len(),
            t.decode_errors
        );
    }
    println!("{}", color::bold("system track:"));
    let (markers, rec_start, rec_stop, drops, card) =
        events.iter().fold((0, 0, 0, 0, 0), |acc, ev| match ev {
            SystemEvent::Marker { .. } => (acc.0 + 1, acc.1, acc.2, acc.3, acc.4),
            SystemEvent::RecordingStart { .. } => (acc.0, acc.1 + 1, acc.2, acc.3, acc.4),
            SystemEvent::RecordingStop { .. } => (acc.0, acc.1, acc.2 + 1, acc.3, acc.4),
            SystemEvent::Dropped { .. } => (acc.0, acc.1, acc.2, acc.3 + 1, acc.4),
            SystemEvent::CardEvent { .. } => (acc.0, acc.1, acc.2, acc.3, acc.4 + 1),
            SystemEvent::SessionStart { .. } => acc,
        });
    println!(
        "  {markers} marker(s), {rec_start} recording(s) started, {rec_stop} stopped, {drops} drop event(s), {card} card event(s)"
    );
    Ok(())
}

pub fn export(
    path: &Path,
    track: Option<&str>,
    format: Format,
    out: Option<PathBuf>,
) -> Result<()> {
    let (log, bundles) = load(path)?;
    let progress = spinner("decoding");
    let tracks = decode::tracks(&log, &bundles);
    progress.finish_and_clear();

    let ext = match format {
        Format::Csv => "csv",
        Format::Mcap => "mcap",
        Format::Mf4 => "mf4",
    };

    if let Some(spec) = track {
        let t = find_track(&tracks, spec)?;
        let out = out.unwrap_or_else(|| PathBuf::from(format!("{}.{ext}", sanitize(&t.name))));
        write_one(t, format, &out)?;
        println!("{} -> {}", t.name, out.display());
        return Ok(());
    }

    match format {
        Format::Mcap => {
            let out = out.unwrap_or_else(|| {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("log");
                PathBuf::from(format!("{stem}.mcap"))
            });
            let file =
                fs::File::create(&out).with_context(|| format!("creating {}", out.display()))?;
            mcap_export::write_tracks(&tracks, std::io::BufWriter::new(file))?;
            println!("{} track(s) -> {}", tracks.len(), out.display());
        }
        Format::Csv | Format::Mf4 => {
            let dir = out.unwrap_or_else(|| PathBuf::from("."));
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let bar = ProgressBar::new(tracks.len() as u64);
            bar.set_style(progress_style());
            let mut written = 0;
            for t in &tracks {
                if t.ty.is_none() || t.items.is_empty() {
                    bar.inc(1);
                    continue;
                }
                let file = dir.join(format!("{}.{ext}", sanitize(&t.name)));
                write_one(t, format, &file)?;
                written += 1;
                bar.inc(1);
            }
            bar.finish_and_clear();
            println!("{written} track(s) -> {}", dir.display());
        }
    }
    Ok(())
}

fn write_one(track: &Track<'_>, format: Format, out: &Path) -> Result<()> {
    match format {
        Format::Csv => {
            let file =
                fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
            csv_export::write_track_csv(track, std::io::BufWriter::new(file))?;
        }
        Format::Mcap => {
            let file =
                fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
            mcap_export::write_tracks(std::slice::from_ref(track), std::io::BufWriter::new(file))?;
        }
        Format::Mf4 => mf4_export::export_track(track, out)?,
    }
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect()
}

pub fn cat(path: &Path, track: &str) -> Result<()> {
    let (log, bundles) = load(path)?;
    let tracks = decode::tracks(&log, &bundles);
    let t = find_track(&tracks, track)?;
    let mut out = std::io::stdout().lock();
    for item in &t.items {
        let t_ps = item.head.t.as_ref().and_then(logring::reader::time_ps);
        let json = serde_json::json!({
            "pos": item.head.pos,
            "t_ps": t_ps,
            "value": logring_export::value_json::value_to_json(&item.value),
        });
        // a reader closing early (`| head`) ends output quietly, like most Unix tools
        if let Err(e) = writeln!(out, "{json}") {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(e.into());
        }
    }
    Ok(())
}

/// `true` when nothing is wrong (exit 0); `false` on torn blocks/records, a bundle that didn't decode, or an
/// unresolved track (exit 1, like fsck).
pub fn check(path: &Path) -> Result<bool> {
    let (log, bundles) = load(path)?;
    let tracks = decode::tracks(&log, &bundles);
    let mut ok = true;

    println!("{}", path.display());
    for seg in log.scan.data.iter().chain(log.scan.meta.iter()) {
        if seg.torn_block || seg.torn_record {
            ok = false;
            println!(
                "  {} segment {}: {}{}",
                color::bad("torn"),
                seg.seq,
                if seg.torn_block { "block " } else { "" },
                if seg.torn_record { "record" } else { "" }
            );
        }
    }
    for hash in log.bundles.keys() {
        if !bundles.contains_key(hash) {
            ok = false;
            println!("  {} bundle {hash:016x} didn't decode", color::bad("error"));
        }
    }
    for t in &tracks {
        if t.ty.is_none() {
            ok = false;
            println!(
                "  {} track {} has no resolved type",
                color::bad("error"),
                t.name
            );
        }
        if t.decode_errors > 0 {
            ok = false;
            println!(
                "  {} track {}: {} record(s) didn't decode",
                color::bad("error"),
                t.name,
                t.decode_errors
            );
        }
    }
    if ok {
        println!("  {}", color::good("clean"));
    }
    Ok(ok)
}

pub fn make_image(path: &Path, n_tracks: u32, records: u64) -> Result<()> {
    crate::make_image::run(path, n_tracks, records)
}

fn spinner(msg: &str) -> ProgressBar {
    let bar = ProgressBar::new_spinner();
    bar.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
    bar.set_message(msg.to_string());
    bar.enable_steady_tick(std::time::Duration::from_millis(100));
    bar
}

fn progress_style() -> ProgressStyle {
    ProgressStyle::with_template("{bar:40} {pos}/{len} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_bar())
}
