//! `size-report`: builds the size bins of `size/` for every MCU target and profile and prints, as Markdown,
//! flash and static RAM per bin, flash per crate, source file and function (from the line tables, so code LTO
//! inlined into a caller still counts for the crate and function it came from) and the RAM of the types and
//! futures (rustc's `-Zprint-type-sizes`, nightly). `just size` runs `size-report all`.

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The MCU targets, with what they stand for.
const TARGETS: &[(&str, &str)] = &[
    ("thumbv6m-none-eabi", "M0+"),
    ("thumbv7em-none-eabihf", "M4/M7"),
    ("thumbv8m.main-none-eabihf", "M33"),
    ("riscv32imac-unknown-none-elf", "RV imac"),
    ("riscv32imc-unknown-none-elf", "RV imc"),
];

/// Cargo profiles of `size/Cargo.toml` and their opt-level.
const PROFILES: &[(&str, &str)] = &[("release", "s"), ("release-z", "z")];

/// The bins, with what they use.
const BINS: &[(&str, &str)] = &[
    (
        "baseline",
        "dummy card, executor, inputs: subtracted from the others",
    ),
    (
        "raw_writer",
        "writer on a raw medium: mount + recovery, push, markers, recordings, flushes",
    ),
    (
        "raw_store",
        "raw_writer + a reliable stream on a TrackStore (reads the ring back)",
    ),
    (
        "exfat_writer",
        "exFAT mount check (logring.wlr binding) + raw_writer's loop",
    ),
    (
        "exfat_store",
        "exfat_writer + a reliable stream on a TrackStore",
    ),
    (
        "exfat_recreate",
        "exfat_store + re-create the ring file on an unbound card, restore its attributes",
    ),
    (
        "full",
        "exfat_recreate + the card check and format (format_card)",
    ),
    (
        "reader",
        "reader only: open, recordings, summary, seek by track and time",
    ),
];

/// The target and profile the per-crate and per-type tables are made for.
const DETAIL_TARGET: &str = "thumbv8m.main-none-eabihf";

#[derive(Parser)]
#[command(name = "size-report", version = version(), about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Everything: the matrix, then the per-crate and per-type tables for the bins on thumbv8m.main, opt s.
    All {
        /// Functions to list per bin.
        #[arg(long, default_value_t = 25)]
        top: usize,
    },
    /// Build every bin for every target and profile; flash and static RAM.
    Matrix,
    /// Flash of one built bin per crate, source file and function.
    Attribute {
        #[arg(long, default_value = DETAIL_TARGET, value_parser = target_names())]
        target: String,
        #[arg(long, default_value = "release", value_parser = profile_names())]
        profile: String,
        #[arg(long, value_parser = bin_names())]
        bin: String,
        /// Functions to list.
        #[arg(long, default_value_t = 40)]
        top: usize,
    },
    /// RAM of logring's types and futures in one bin (nightly rustc's -Zprint-type-sizes).
    Types {
        #[arg(long, default_value = DETAIL_TARGET, value_parser = target_names())]
        target: String,
        #[arg(long, value_parser = bin_names())]
        bin: String,
    },
    /// Print shell completion for the given shell.
    Completions { shell: clap_complete::Shell },
}

fn version() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("GIT_SHA"),
        ", commit ",
        env!("COMMIT_TIME"),
        ")"
    )
}

fn target_names() -> clap::builder::PossibleValuesParser {
    TARGETS
        .iter()
        .map(|(t, h)| clap::builder::PossibleValue::new(*t).help(*h))
        .collect::<Vec<_>>()
        .into()
}

fn profile_names() -> clap::builder::PossibleValuesParser {
    PROFILES
        .iter()
        .map(|(p, o)| clap::builder::PossibleValue::new(*p).help(format!("opt-level {o}")))
        .collect::<Vec<_>>()
        .into()
}

fn bin_names() -> clap::builder::PossibleValuesParser {
    BINS.iter()
        .map(|(b, h)| clap::builder::PossibleValue::new(*b).help(*h))
        .collect::<Vec<_>>()
        .into()
}

fn main() -> Result<()> {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    match Cli::parse().cmd {
        Cmd::All { top } => {
            print!("{}", matrix()?);
            for (bin, _) in BINS.iter().filter(|(b, _)| *b != "baseline") {
                println!(
                    "\n## {bin}: flash by crate, file and function ({DETAIL_TARGET}, opt s)\n"
                );
                print!("{}", attribute(&elf(DETAIL_TARGET, "release", bin), top)?);
                println!("\n## {bin}: RAM of types and futures ({DETAIL_TARGET})\n");
                print!("{}", types(DETAIL_TARGET, bin)?);
            }
        }
        Cmd::Matrix => print!("{}", matrix()?),
        Cmd::Attribute {
            target,
            profile,
            bin,
            top,
        } => {
            build(&target, &profile)?;
            print!("{}", attribute(&elf(&target, &profile, &bin), top)?)
        }
        Cmd::Types { target, bin } => print!("{}", types(&target, &bin)?),
        Cmd::Completions { shell } => clap_complete::generate(
            shell,
            &mut Cli::command(),
            "size-report",
            &mut std::io::stdout(),
        ),
    }
    Ok(())
}

/// The `size/` crate.
fn size_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn elf(target: &str, profile: &str, bin: &str) -> PathBuf {
    size_dir()
        .join("target")
        .join(target)
        .join(profile)
        .join(bin)
}

/// Build all bins of `size/` for `target` with `profile`.
fn build(target: &str, profile: &str) -> Result<()> {
    let out = Command::new("cargo")
        .args([
            "build",
            "--quiet",
            "--bins",
            "--profile",
            profile,
            "--target",
            target,
        ])
        .current_dir(size_dir())
        .output()
        .context("running cargo")?;
    if !out.status.success() {
        bail!(
            "cargo build --profile {profile} --target {target} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Section sizes of an ELF.
#[derive(Default, Clone, Copy)]
struct Sections {
    text: u64,
    rodata: u64,
    data: u64,
    bss: u64,
}

impl Sections {
    fn flash(&self) -> u64 {
        self.text + self.rodata + self.data
    }

    fn ram(&self) -> u64 {
        self.data + self.bss
    }
}

fn sections(path: &Path) -> Result<Sections> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let obj = object::File::parse(&*bytes)?;
    let size = |name: &str| obj.section_by_name(name).map_or(0, |s| s.size());
    Ok(Sections {
        text: size(".text"),
        rodata: size(".rodata"),
        data: size(".data"),
        bss: size(".bss"),
    })
}

fn kib(b: u64) -> String {
    format!("{:.1}", b as f64 / 1024.0)
}

/// Build everything; tables of flash (minus baseline) and static RAM per bin, target and profile.
fn matrix() -> Result<String> {
    let jobs: Vec<_> = PROFILES
        .iter()
        .flat_map(|(p, _)| TARGETS.iter().map(move |(t, _)| (*t, *p)))
        .collect();
    let bar = ProgressBar::new(jobs.len() as u64).with_style(
        ProgressStyle::with_template("{spinner} building {msg} [{bar:30}] {pos}/{len} {elapsed}")?
            .progress_chars("=> "),
    );
    let mut got: HashMap<(&str, &str, &str), Sections> = HashMap::new();
    for (t, p) in &jobs {
        bar.set_message(format!("{t} {p}"));
        build(t, p)?;
        for (b, _) in BINS {
            got.insert((t, p, b), sections(&elf(t, p, b))?);
        }
        bar.inc(1);
    }
    bar.finish_and_clear();

    let mut s = String::new();
    let head = |s: &mut String| {
        s.push_str("| bin |");
        for (_, h) in TARGETS {
            write!(s, " {h} |").unwrap();
        }
        s.push_str("\n|---|");
        s.push_str(&"---:|".repeat(TARGETS.len()));
        s.push('\n');
    };
    for (p, o) in PROFILES {
        writeln!(
            s,
            "## Flash, KiB (text + rodata + data), minus baseline, opt-level {o}\n"
        )
        .unwrap();
        head(&mut s);
        for (b, _) in BINS {
            write!(s, "| {b} |").unwrap();
            for (t, _) in TARGETS {
                let x = got[&(*t, *p, *b)];
                let v = if *b == "baseline" {
                    x.flash()
                } else {
                    x.flash() - got[&(*t, *p, "baseline")].flash()
                };
                write!(s, " {} |", kib(v)).unwrap();
            }
            s.push('\n');
        }
        s.push('\n');
    }
    writeln!(
        s,
        "## Flash split, opt-level s, {DETAIL_TARGET}: text / rodata, bytes, minus baseline\n"
    )
    .unwrap();
    s.push_str("| bin | text | rodata | static RAM (data + bss) |\n|---|---:|---:|---:|\n");
    let base = got[&(DETAIL_TARGET, "release", "baseline")];
    for (b, _) in BINS.iter().skip(1) {
        let x = got[&(DETAIL_TARGET, "release", *b)];
        writeln!(
            s,
            "| {b} | {} | {} | {} |",
            x.text - base.text,
            x.rodata - base.rodata,
            x.ram()
        )
        .unwrap();
    }
    s.push_str("\nBins:\n\n");
    for (b, h) in BINS {
        writeln!(s, "- `{b}`: {h}").unwrap();
    }
    Ok(s)
}

/// Who a byte of code belongs to.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Owner {
    krate: String,
    file: String,
    func: String,
}

/// Flash of one ELF's `.text` per crate, source file and function, and its named `.rodata` objects.
fn attribute(path: &Path, top: usize) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let obj = object::File::parse(&*bytes)?;
    let text = obj.section_by_name(".text").context("no .text")?;
    let (lo, len) = (text.address(), text.size());
    let loader = addr2line::Loader::new(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut crates = CrateOf::default();

    let mut owners: Vec<Owner> = Vec::new();
    let mut index: HashMap<Owner, u32> = HashMap::new();
    let mut intern = |o: Owner| -> u32 {
        *index.entry(o.clone()).or_insert_with(|| {
            owners.push(o);
            owners.len() as u32 - 1
        })
    };
    let mut by_byte: Vec<Option<u32>> = vec![None; len as usize];

    // code with line info: its innermost source location and function (inlined code counts where it came from)
    let ranges = loader
        .find_location_range(lo, lo + len)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for (addr, n, loc) in ranges {
        let Some(file) = loc.file else { continue };
        let (krate, rel) = crates.of(file);
        let func = innermost(&loader, addr).unwrap_or_else(|| "?".into());
        let id = intern(Owner {
            krate,
            file: rel,
            func,
        });
        let from = addr.saturating_sub(lo) as usize;
        let to = ((addr + n).saturating_sub(lo) as usize).min(by_byte.len());
        for b in &mut by_byte[from.min(to)..to] {
            *b = Some(id);
        }
    }
    // the rest (compiler_builtins and other code without line info): by symbol
    let mut syms: Vec<(u64, u64, String)> = obj
        .symbols()
        .filter(|s| s.kind() == SymbolKind::Text && s.size() > 0)
        .map(|s| {
            (
                s.address() & !1,
                s.size(),
                s.name().unwrap_or("?").to_string(),
            )
        })
        .collect();
    syms.sort();
    for (a, n, name) in &syms {
        let name = addr2line::demangle_auto(name.as_str().into(), None).to_string();
        let krate = name
            .trim_start_matches('<')
            .split("::")
            .next()
            .unwrap_or("?")
            .to_string();
        let id = intern(Owner {
            krate,
            file: "(no line info)".into(),
            func: shorten(&name),
        });
        let from = a.saturating_sub(lo) as usize;
        let to = ((a + n).saturating_sub(lo) as usize).min(by_byte.len());
        for b in &mut by_byte[from.min(to)..to] {
            b.get_or_insert(id);
        }
    }

    let mut per: HashMap<u32, u64> = HashMap::new();
    let mut unowned = 0u64;
    for b in &by_byte {
        match b {
            Some(id) => *per.entry(*id).or_default() += 1,
            None => unowned += 1,
        }
    }
    let mut krate: HashMap<&str, u64> = HashMap::new();
    let mut file: HashMap<(&str, &str), u64> = HashMap::new();
    let mut func: HashMap<(&str, &str), u64> = HashMap::new();
    for (id, n) in &per {
        let o = &owners[*id as usize];
        *krate.entry(&o.krate).or_default() += n;
        *file.entry((&o.krate, &o.file)).or_default() += n;
        *func.entry((&o.krate, &o.func)).or_default() += n;
    }

    let mut s = String::new();
    writeln!(
        s,
        ".text {} bytes; by crate (code inlined elsewhere counts for its own crate):\n",
        len
    )
    .unwrap();
    s.push_str("| crate | bytes | % |\n|---|---:|---:|\n");
    for (k, n) in sorted(krate) {
        writeln!(s, "| {k} | {n} | {:.1} |", n as f64 * 100.0 / len as f64).unwrap();
    }
    if unowned > 0 {
        writeln!(
            s,
            "| (padding, literal pools) | {unowned} | {:.1} |",
            unowned as f64 * 100.0 / len as f64
        )
        .unwrap();
    }
    s.push_str("\nBy source file (logring, exfat_lite and wire_weaver crates; others summed per crate above):\n\n");
    s.push_str("| file | bytes |\n|---|---:|\n");
    for ((k, f), n) in sorted(file) {
        if is_ours(k) {
            writeln!(s, "| {k}/{f} | {n} |").unwrap();
        }
    }
    writeln!(s, "\nTop {top} functions (the source function a byte came from, inlined or not; async bodies under their fn's name):\n").unwrap();
    s.push_str("| crate | function | bytes |\n|---|---|---:|\n");
    for ((k, f), n) in sorted(func).into_iter().take(top) {
        writeln!(s, "| {k} | `{}` | {n} |", f.replace('|', "\\|")).unwrap();
    }

    let mut ro: Vec<(u64, String)> = obj
        .symbols()
        .filter(|s| s.kind() == SymbolKind::Data && s.size() >= 64)
        .filter(|s| {
            obj.section_by_index(s.section_index().unwrap_or(object::SectionIndex(0)))
                .is_ok_and(|x| x.name() == Ok(".rodata"))
        })
        .map(|s| {
            (
                s.size(),
                shorten(&addr2line::demangle_auto(
                    s.name().unwrap_or("?").into(),
                    None,
                )),
            )
        })
        .collect();
    ro.sort_by(|a, b| b.cmp(a));
    if !ro.is_empty() {
        s.push_str(
            "\nNamed .rodata objects of 64 bytes and more:\n\n| object | bytes |\n|---|---:|\n",
        );
        for (n, name) in ro.iter().take(15) {
            writeln!(s, "| `{name}` | {n} |").unwrap();
        }
    }
    Ok(s)
}

fn sorted<K: Ord>(m: HashMap<K, u64>) -> Vec<(K, u64)> {
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn is_ours(krate: &str) -> bool {
    krate.starts_with("ww_")
        || krate.starts_with("logring")
        || krate == "exfat_lite"
        || krate == "shrink_wrap"
        || krate == "wire_weaver"
}

/// The innermost function at `addr` (an inlined one if it is inlined there), shortened.
fn innermost(loader: &addr2line::Loader, addr: u64) -> Option<String> {
    let mut frames = loader.find_frames(addr).ok()?;
    let f = frames.next().ok()??;
    let name = f.function?.demangle().ok()?.to_string();
    Some(shorten(&name))
}

/// `ww_logring::writer::Logring<ww_logring::exfat::ring::ExfatRing<logring_size::Card>, 4096: usize, 8:
/// usize>::mount::{closure#0}` -> `Logring<ExfatRing<Card>, 4096, 8>::mount`: module paths, const types,
/// closure suffixes and hashes dropped.
fn shorten(name: &str) -> String {
    let mut name = name
        .replace(": usize", "")
        .replace("'_, ", "")
        .replace("{async fn body of ", "{async ");
    while let Some(i) = name.find("::{closure#").or_else(|| name.find("::{shim:")) {
        let j = name[i..].find('}').map_or(name.len(), |j| i + j + 1);
        name.replace_range(i..j, "");
    }
    if let Some(i) = name.rfind("::h")
        && name[i + 3..].len() == 16
        && name[i + 3..].chars().all(|c| c.is_ascii_hexdigit())
    {
        name.truncate(i);
    }
    let mut out = String::with_capacity(name.len());
    let mut word = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        // a lower-case word followed by `::` and a name is a module (or crate): drop it with its `::`
        if c == ':' && chars.peek() == Some(&':') && word.starts_with(|w: char| w.is_lowercase()) {
            let mut ahead = chars.clone();
            ahead.next();
            if ahead
                .peek()
                .is_some_and(|n| n.is_alphanumeric() || *n == '_')
            {
                chars.next();
                word.clear();
                continue;
            }
        }
        out.push_str(&word);
        word.clear();
        out.push(c);
    }
    out.push_str(&word);
    out
}

/// Which crate a source file belongs to: the package of the nearest Cargo.toml up from it, or the standard
/// library's crate for `/rustc/...` paths.
#[derive(Default)]
struct CrateOf {
    roots: HashMap<PathBuf, Option<String>>,
}

impl CrateOf {
    /// Crate and the file's path inside it.
    fn of(&mut self, file: &str) -> (String, String) {
        if let Some(i) = file.find("/library/") {
            let rest = &file[i + 9..];
            let (k, f) = rest.split_once('/').unwrap_or((rest, ""));
            return (k.to_string(), f.to_string());
        }
        let mut dir = Path::new(file).parent();
        while let Some(d) = dir {
            let name = self
                .roots
                .entry(d.to_path_buf())
                .or_insert_with(|| package_name(&d.join("Cargo.toml")))
                .clone();
            if let Some(n) = name {
                let rel = Path::new(file).strip_prefix(d).unwrap_or(Path::new(file));
                return (n, rel.display().to_string());
            }
            dir = d.parent();
        }
        ("?".into(), file.to_string())
    }
}

/// `name` of the `[package]` in a Cargo.toml, if there is one.
fn package_name(toml: &Path) -> Option<String> {
    let text = std::fs::read_to_string(toml).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(v) = line.strip_prefix("name") {
            let v = v.trim_start().strip_prefix('=')?.trim().trim_matches('"');
            return Some(v.to_string());
        }
    }
    None
}

/// A type's name, size and its fields' names and sizes.
type TypeSize = (String, u64, Vec<(String, u64)>);

/// RAM of logring's types and futures in `bin` on `target`: nightly rustc's `-Zprint-type-sizes` on a fresh
/// build of the size crate (the dependencies' generic types are laid out in it, where they are instantiated).
fn types(target: &str, bin: &str) -> Result<String> {
    let dir = size_dir().join("target").join("types");
    let clean = Command::new("cargo")
        .args([
            "+nightly",
            "clean",
            "--quiet",
            "-p",
            "logring_size",
            "--release",
            "--target",
            target,
        ])
        .arg("--target-dir")
        .arg(&dir)
        .current_dir(size_dir())
        .status()?;
    if !clean.success() {
        bail!("cargo +nightly clean failed");
    }
    let out = Command::new("cargo")
        .args([
            "+nightly",
            "rustc",
            "--quiet",
            "--release",
            "--target",
            target,
            "--bin",
            bin,
        ])
        .arg("--target-dir")
        .arg(&dir)
        .args(["--", "-Zprint-type-sizes"])
        .current_dir(size_dir())
        .output()
        .context("running cargo +nightly")?;
    if !out.status.success() {
        bail!(
            "cargo +nightly rustc failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // `print-type-size type: `NAME`: N bytes, alignment: A bytes` and its field lines
    let mut all: Vec<TypeSize> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("print-type-size type: `") {
            let Some((name, tail)) = rest.rsplit_once("`: ") else {
                continue;
            };
            let n = tail
                .split(' ')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            all.push((name.to_string(), n, Vec::new()));
        } else if let Some(rest) = line.strip_prefix("print-type-size     field `.")
            && let Some((f, tail)) = rest.split_once("`: ")
            && let Some(last) = all.last_mut()
        {
            let n = tail
                .split(' ')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            last.2.push((f.to_string(), n));
        }
    }
    const WRAPPERS: &[&str] = &[
        "core::",
        "logring_size::Slot",
        "embassy_sync::mutex::Mutex",
        "ww_logring::Error",
        "ww_logring::exfat::Opened",
    ];
    let ours = |n: &str| {
        let n = n.strip_prefix("{async fn body of ").unwrap_or(n);
        (n.starts_with("ww_logring::")
            || n.starts_with("exfat_lite::")
            || n.starts_with("<ww_logring::")
            || n.starts_with("{async block@src/bin/"))
            && !WRAPPERS.iter().any(|w| n.starts_with(w))
    };
    let mut seen = std::collections::HashSet::new();
    let mut picked: Vec<_> = all
        .iter()
        .filter(|(n, size, _)| *size >= 96 && ours(n) && seen.insert(n.clone()))
        .collect();
    picked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut s = String::new();
    s.push_str("| type or future | bytes |\n|---|---:|\n");
    for (n, size, _) in picked.iter().take(30) {
        writeln!(s, "| `{}` | {size} |", shorten(n).replace('|', "\\|")).unwrap();
    }
    if let Some((n, size, fields)) = all
        .iter()
        .find(|(n, _, _)| n.starts_with("ww_logring::Logring<"))
    {
        writeln!(s, "\n`{}`, {size} bytes, by field:\n", shorten(n)).unwrap();
        s.push_str("| field | bytes |\n|---|---:|\n");
        let mut f = fields.clone();
        f.sort_by_key(|a| std::cmp::Reverse(a.1));
        for (name, n) in f {
            writeln!(s, "| `{name}` | {n} |").unwrap();
        }
    }
    Ok(s)
}
