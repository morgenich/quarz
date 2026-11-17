use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use quarz::ops::{self, CreateOptions};
use quarz::progress::Progress;
use quarz::unpack::{ExtractOptions, Overwrite};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "qz", version, about = "Quarz-Archivierer")]
struct Cli {
    #[arg(long, global = true)]
    quiet: bool,
    #[arg(long, global = true)]
    threads: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(alias = "a")]
    Add {
        archive: PathBuf,
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(short = 'm', long)]
        level: Option<u8>,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
        #[arg(short, long)]
        comment: Option<String>,
        #[arg(short = 'v', long)]
        volume: Option<String>,
        #[arg(long = "rr", value_name = "PROZENT")]
        recovery: Option<u8>,
        #[arg(long)]
        sfx: bool,
        #[arg(long)]
        force: bool,
    },
    #[command(alias = "x")]
    Extract {
        archive: PathBuf,
        patterns: Vec<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long, conflicts_with = "overwrite")]
        skip: bool,
    },
    #[command(alias = "l")]
    List {
        archive: PathBuf,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    #[command(alias = "t")]
    Test {
        archive: PathBuf,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    #[command(alias = "d")]
    Delete {
        archive: PathBuf,
        #[arg(required = true)]
        patterns: Vec<String>,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    #[command(alias = "c")]
    Comment {
        archive: PathBuf,
        text: String,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    #[command(alias = "k")]
    Lock {
        archive: PathBuf,
        #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    #[command(alias = "rr")]
    Protect {
        archive: PathBuf,
        #[arg(default_value_t = 5)]
        percent: u8,
    },
    #[command(alias = "r")]
    Repair {
        archive: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    #[command(alias = "s")]
    Sfx {
        archive: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    #[command(alias = "j")]
    Join {
        part: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn with_password<T>(given: Option<String>, mut action: impl FnMut(Option<&str>) -> Result<T>) -> Result<T> {
    let mut password = ask_password(given, false)?;
    loop {
        match action(password.as_deref()) {
            Err(e) if password.is_none() && e.downcast_ref::<quarz::PasswordNeeded>().is_some() => {
                password = Some(rpassword::prompt_password("Passwort: ")?);
            }
            other => return other,
        }
    }
}

struct Meter {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Meter {
    fn start(label: &'static str, progress: Arc<Progress>, quiet: bool) -> Meter {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = (!quiet).then(|| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(300));
                    eprint!("\r{label} {:5.1} %   ", progress.fraction() * 100.0);
                    let _ = std::io::stderr().flush();
                }
                eprint!("\r{}\r", " ".repeat(40));
            })
        });
        Meter { stop, thread }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn ask_password(given: Option<String>, needed: bool) -> Result<Option<String>> {
    match given {
        Some(p) if !p.is_empty() => Ok(Some(p)),
        Some(_) => Ok(Some(rpassword::prompt_password("Passwort: ")?)),
        None if needed => Ok(Some(rpassword::prompt_password("Passwort: ")?)),
        None => Ok(None),
    }
}

fn password_for(archive: &Path, given: Option<String>) -> Result<Option<String>> {
    ask_password(given, ops::is_encrypted(archive)?)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn sfx_stub() -> Result<PathBuf> {
    if let Some(stub) = std::env::var_os("QZ_SFX_STUB") {
        return Ok(PathBuf::from(stub));
    }
    let exe = std::env::current_exe()?;
    let window = exe.with_file_name("quarz.exe");
    Ok(if window.is_file() { window } else { exe })
}

fn print_pack_stats(stats: &quarz::pack::Stats, size: u64, started: Instant) {
    println!(
        "{} Dateien, {:.1} MB -> {:.1} MB ({:.1} %) in {:.1} s",
        stats.files,
        mb(stats.bytes_in),
        mb(size),
        100.0 * size as f64 / stats.bytes_in.max(1) as f64,
        started.elapsed().as_secs_f64()
    );
    println!(
        "ausgepackte Stroeme: {} ({:.1} MB Rohdaten), doppelt gespart: {:.1} MB",
        stats.unwrapped,
        mb(stats.unwrapped_bytes),
        mb(stats.dedup_bytes)
    );
    if std::env::var_os("QZ_TRACE").is_some() {
        println!(
            "Zeiten: lesen+auspacken {:.1} s, einsortieren {:.1} s, Rest komprimieren {:.1} s",
            stats.load_s, stats.intern_s, stats.finish_s
        );
    }
    for skipped in &stats.skipped {
        println!("UEBERSPRUNGEN: {skipped}");
    }
}

fn archive_size(archive: &Path) -> u64 {
    ops::open_io(archive).map(|(_, total, _)| total).unwrap_or(0)
}

fn run(cli: Cli) -> Result<i32> {
    let threads = cli.threads.unwrap_or_else(ops::default_threads).max(1);
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().ok();
    let progress = Arc::new(Progress::default());
    let started = Instant::now();
    let quiet = cli.quiet;
    match cli.command {
        Command::Add { archive, inputs, level, password, comment, volume, recovery, sfx, force } => {
            let update = archive.exists() && !force;
            if update {
                if volume.is_some() || sfx || comment.is_some() || recovery.is_some() {
                    bail!("{} existiert schon: -v, --rr, --sfx und -c gelten nur fuer neue Archive", archive.display());
                }
                let password = password_for(&archive, password)?;
                let stats = {
                    let _meter = Meter::start("Fuege hinzu", progress.clone(), quiet);
                    ops::add(&archive, &inputs, password.as_deref(), level, threads, &progress)?
                };
                print_pack_stats(&stats, archive_size(&archive), started);
                return Ok(if stats.skipped.is_empty() { 0 } else { 1 });
            }
            let options = CreateOptions {
                level: level.unwrap_or(3).min(5),
                threads,
                password: ask_password(password, false)?,
                comment: comment.unwrap_or_default(),
                volume_size: volume.as_deref().map(ops::parse_size).transpose()?,
                recovery: recovery.unwrap_or(0).min(100),
                sfx: if sfx { Some(sfx_stub()?) } else { None },
            };
            let stats = {
                let _meter = Meter::start("Packe", progress.clone(), quiet);
                ops::create(&archive, &inputs, &options, &progress)?
            };
            let first = if options.volume_size.is_some() { quarz::volume::part_name(&archive, 1) } else { archive.clone() };
            print_pack_stats(&stats, archive_size(&first), started);
            Ok(if stats.skipped.is_empty() { 0 } else { 1 })
        }
        Command::Extract { archive, patterns, output, password, overwrite, skip } => {
            let dest = output.unwrap_or_else(|| default_target(&archive));
            let overwrite = if overwrite {
                Overwrite::Replace
            } else if skip {
                Overwrite::Skip
            } else {
                Overwrite::Fail
            };
            let failed = with_password(password, |password| {
                let _meter = Meter::start("Entpacke", progress.clone(), quiet);
                ops::extract_any(&archive, password, Some(&dest), &patterns, overwrite, 0, &progress)
            })?;
            report("entpackt", &progress, &failed, started)
        }
        Command::Test { archive, password } => {
            let failed = with_password(password, |password| {
                let _meter = Meter::start("Pruefe", progress.clone(), quiet);
                ops::extract_any(&archive, password, None, &[], Overwrite::Fail, 0, &progress)
            })?;
            let code = report("geprueft", &progress, &failed, started)?;
            if ops::is_quarz(&archive) {
                if let Some(check) = ops::check_recovery(&archive, &Progress::default())? {
                    println!(
                        "Wiederherstellungsdaten {} %: {} von {} Teilen beschaedigt{}",
                        check.percent,
                        check.damaged,
                        check.data_shards,
                        if check.lost_stripes > 0 { ", nicht mehr reparierbar" } else { "" }
                    );
                }
            }
            Ok(code)
        }
        Command::List { archive, password } => {
            let listing = with_password(password, |password| ops::listing(&archive, password))?;
            let mut files: Vec<_> = listing.entries.iter().filter(|e| !e.is_dir).collect();
            files.sort_by_key(|f| f.path.to_lowercase());
            for file in &files {
                println!("{:>14}  {}", file.size, file.path);
            }
            let size: u64 = files.iter().map(|f| f.size).sum();
            let mut notes = vec![listing.kind.clone()];
            notes.extend(listing.notes.iter().cloned());
            println!(
                "{} Dateien, {:.1} MB, im Archiv {:.1} MB ({:.1} %), {}",
                files.len(),
                mb(size),
                mb(listing.packed),
                100.0 * listing.packed as f64 / size.max(1) as f64,
                notes.join(", ")
            );
            if !listing.comment.is_empty() {
                println!("Kommentar: {}", listing.comment);
            }
            Ok(0)
        }
        Command::Delete { archive, patterns, password } => {
            let password = password_for(&archive, password)?;
            let removed = ops::delete(&archive, &patterns, password.as_deref(), &progress)?;
            println!("{removed} Dateien entfernt");
            Ok(0)
        }
        Command::Comment { archive, text, password } => {
            let password = password_for(&archive, password)?;
            ops::set_comment(&archive, &text, password.as_deref(), &progress)?;
            println!("Kommentar gesetzt");
            Ok(0)
        }
        Command::Lock { archive, password } => {
            let password = password_for(&archive, password)?;
            ops::lock(&archive, password.as_deref(), &progress)?;
            println!("Archiv gesperrt");
            Ok(0)
        }
        Command::Protect { archive, percent } => {
            {
                let _meter = Meter::start("Berechne", progress.clone(), quiet);
                ops::protect(&archive, percent.clamp(1, 100), &progress)?;
            }
            println!("Wiederherstellungsdaten hinzugefuegt, Archiv jetzt {:.1} MB", mb(archive_size(&archive)));
            Ok(0)
        }
        Command::Repair { archive, output } => {
            let output = output.unwrap_or_else(|| {
                let stem = archive.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                archive.with_file_name(format!("{stem}.repariert.qz"))
            });
            let result = {
                let _meter = Meter::start("Repariere", progress.clone(), quiet);
                ops::repair(&archive, &output, &progress)?
            };
            println!(
                "{} von {} Teilen beschaedigt, {} repariert -> {}",
                result.damaged,
                result.data_shards,
                result.repaired,
                output.display()
            );
            if result.lost_stripes > 0 {
                println!("FEHLER: zu viel Schaden fuer die vorhandenen Wiederherstellungsdaten");
                return Ok(2);
            }
            Ok(0)
        }
        Command::Sfx { archive, output } => {
            let output = output.unwrap_or_else(|| archive.with_extension("exe"));
            ops::make_sfx(&archive, &sfx_stub()?, &output)?;
            println!("{} angelegt ({:.1} MB)", output.display(), mb(std::fs::metadata(&output)?.len()));
            Ok(0)
        }
        Command::Join { part, output } => {
            let output = match output {
                Some(output) => output,
                None => quarz::volume::parts_of(&part).map(|(base, _)| base).unwrap_or_else(|| part.with_extension("ganz.qz")),
            };
            let count = ops::join(&part, &output)?;
            println!("{count} Teile zusammengefuegt -> {} ({:.1} MB)", output.display(), mb(std::fs::metadata(&output)?.len()));
            Ok(0)
        }
    }
}

fn default_target(archive: &Path) -> PathBuf {
    let base = quarz::volume::parts_of(archive).map(|(base, _)| base).unwrap_or_else(|| archive.to_path_buf());
    PathBuf::from(base.file_stem().unwrap_or_default())
}

fn report(verb: &str, progress: &Progress, failed: &[String], started: Instant) -> Result<i32> {
    let total = progress.total.load(Ordering::Relaxed);
    println!("{:.1} MB {verb} in {:.1} s", mb(total), started.elapsed().as_secs_f64());
    for item in failed {
        println!("FEHLER: {item}");
    }
    Ok(if failed.is_empty() { 0 } else { 2 })
}

fn self_extract(exe: &Path) -> Result<i32> {
    let dest = exe.with_extension("");
    println!("Quarz-Selbstentpacker: entpacke nach {}", dest.display());
    let password = if ops::is_encrypted(exe)? { Some(rpassword::prompt_password("Passwort: ")?) } else { None };
    let reader = ops::open(exe, password.as_deref())?;
    let progress = Arc::new(Progress::default());
    let options = ExtractOptions { dest: Some(&dest), select: None, overwrite: Overwrite::Skip, strip: 0 };
    let failed = {
        let _meter = Meter::start("Entpacke", progress.clone(), false);
        reader.extract(&options, &progress)?
    };
    report("entpackt", &progress, &failed, Instant::now())
}

fn main() {
    let result = match ops::embedded_archive() {
        Some(exe) if std::env::args().len() == 1 => {
            let result = self_extract(&exe);
            println!("Eingabetaste zum Beenden.");
            let _ = std::io::stdin().read_line(&mut String::new());
            result
        }
        _ => run(Cli::parse()),
    };
    let code = match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Fehler: {e:#}");
            3
        }
    };
    std::process::exit(code);
}
