use crate::PasswordNeeded;
use crate::progress::Progress;
use crate::unpack::{Overwrite, safe_target};
use anyhow::{Context, Result, anyhow, bail};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Zip,
    SevenZ,
    Rar,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    Gz,
    Bz2,
    Xz,
    Zst,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Zip => "ZIP",
            Format::SevenZ => "7z",
            Format::Rar => "RAR",
            Format::Tar => "TAR",
            Format::TarGz => "TAR.GZ",
            Format::TarBz2 => "TAR.BZ2",
            Format::TarXz => "TAR.XZ",
            Format::TarZst => "TAR.ZST",
            Format::Gz => "GZIP",
            Format::Bz2 => "BZIP2",
            Format::Xz => "XZ",
            Format::Zst => "ZSTD",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub mtime: i64,
    pub encrypted: bool,
}

pub fn detect(path: &Path) -> Option<Format> {
    let mut head = [0u8; 512];
    let got = File::open(path).ok()?.read(&mut head).ok()?;
    let head = &head[..got];
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let tarball = |short: &str, long: &str| name.ends_with(short) || name.ends_with(long);
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        Some(Format::Zip)
    } else if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
        Some(Format::SevenZ)
    } else if head.starts_with(b"Rar!\x1a\x07") {
        Some(Format::Rar)
    } else if head.starts_with(b"\x1f\x8b") {
        Some(if tarball(".tgz", ".tar.gz") { Format::TarGz } else { Format::Gz })
    } else if head.starts_with(b"BZh") {
        Some(if tarball(".tbz2", ".tar.bz2") { Format::TarBz2 } else { Format::Bz2 })
    } else if head.starts_with(b"\xfd7zXZ\x00") {
        Some(if tarball(".txz", ".tar.xz") { Format::TarXz } else { Format::Xz })
    } else if head.starts_with(b"\x28\xb5\x2f\xfd") {
        Some(if tarball(".tzst", ".tar.zst") { Format::TarZst } else { Format::Zst })
    } else if head.len() >= 262 && &head[257..262] == b"ustar" {
        Some(Format::Tar)
    } else {
        None
    }
}

fn local_to_unix(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> i64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
        let local = SYSTEMTIME {
            wYear: year,
            wMonth: month as u16,
            wDayOfWeek: 0,
            wDay: day as u16,
            wHour: hour as u16,
            wMinute: minute as u16,
            wSecond: second as u16,
            wMilliseconds: 0,
        };
        let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let mut ft: FILETIME = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            TzSpecificLocalTimeToSystemTime(std::ptr::null(), &local, &mut utc) != 0
                && SystemTimeToFileTime(&utc, &mut ft) != 0
        };
        if !ok {
            return 0;
        }
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        (ticks as i64 - 116_444_736_000_000_000) * 100
    }
    #[cfg(not(windows))]
    {
        let _ = (year, month, day, hour, minute, second);
        0
    }
}

fn dos_to_unix(packed: u32) -> i64 {
    let (date, time) = ((packed >> 16) as u16, packed as u16);
    if date == 0 {
        return 0;
    }
    local_to_unix(
        1980 + (date >> 9),
        ((date >> 5) & 15) as u8,
        (date & 31) as u8,
        (time >> 11) as u8,
        ((time >> 5) & 63) as u8,
        ((time & 31) * 2) as u8,
    )
}

fn system_time_nanos(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

fn clean(name: &str) -> String {
    name.replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string()
}

fn stem_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "inhalt".into())
}

fn is_password_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("password") || lower.contains("passwort") || lower.contains("encrypt")
}

fn decoder(path: &Path, format: Format) -> Result<Box<dyn Read>> {
    let file = BufReader::with_capacity(1 << 20, File::open(path)?);
    Ok(match format {
        Format::Tar => Box::new(file),
        Format::TarGz | Format::Gz => Box::new(flate2::read::MultiGzDecoder::new(file)),
        Format::TarBz2 | Format::Bz2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
        Format::TarXz | Format::Xz => Box::new(liblzma::read::XzDecoder::new_multi_decoder(file)),
        Format::TarZst | Format::Zst => Box::new(zstd::stream::read::Decoder::new(file)?),
        _ => bail!("kein Datenstrom-Format"),
    })
}

fn is_tar(format: Format) -> bool {
    matches!(format, Format::Tar | Format::TarGz | Format::TarBz2 | Format::TarXz | Format::TarZst)
}

fn rar_open<'a>(path: &'a Path, password: Option<&'a str>) -> unrar::Archive<'a> {
    match password {
        Some(password) => unrar::Archive::with_password(path, password),
        None => unrar::Archive::new(path),
    }
}

fn rar_error(e: impl std::fmt::Display, had_password: bool) -> anyhow::Error {
    let text = e.to_string();
    if is_password_error(&text) {
        if had_password { anyhow!("falsches Passwort") } else { anyhow!(PasswordNeeded) }
    } else {
        anyhow!("RAR: {text}")
    }
}

fn sevenz_open(path: &Path, password: Option<&str>) -> Result<sevenz_rust2::ArchiveReader<File>> {
    let pass = password.map(sevenz_rust2::Password::new).unwrap_or_else(sevenz_rust2::Password::empty);
    sevenz_rust2::ArchiveReader::open(path, pass).map_err(|e| {
        let text = format!("{e}");
        if is_password_error(&text) {
            if password.is_some() { anyhow!("falsches Passwort") } else { anyhow!(PasswordNeeded) }
        } else {
            anyhow!("7z: {text}")
        }
    })
}

pub fn list(path: &Path, format: Format, password: Option<&str>) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    match format {
        Format::Zip => {
            let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path)?)).context("ZIP")?;
            for i in 0..archive.len() {
                let file = archive.by_index_raw(i).context("ZIP")?;
                let Ok(name) = file.name() else { continue };
                let mtime = file
                    .last_modified()
                    .map(|t| local_to_unix(t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second()))
                    .unwrap_or(0);
                entries.push(Entry {
                    path: clean(&name),
                    size: file.size(),
                    is_dir: file.is_dir(),
                    mtime,
                    encrypted: file.encrypted(),
                });
            }
        }
        Format::SevenZ => {
            let reader = sevenz_open(path, password)?;
            for file in &reader.archive().files {
                let mtime = if file.has_last_modified_date {
                    system_time_nanos(std::time::SystemTime::from(file.last_modified_date))
                } else {
                    0
                };
                entries.push(Entry {
                    path: clean(file.name()),
                    size: file.size(),
                    is_dir: file.is_directory(),
                    mtime,
                    encrypted: false,
                });
            }
        }
        Format::Rar => {
            let mut archive = rar_open(path, password).open_for_listing().map_err(|e| rar_error(e, password.is_some()))?;
            loop {
                let Some(header) = archive.read_header().map_err(|e| rar_error(e, password.is_some()))? else { break };
                let entry = header.entry();
                entries.push(Entry {
                    path: clean(&entry.filename.to_string_lossy()),
                    size: entry.unpacked_size,
                    is_dir: entry.is_directory(),
                    mtime: dos_to_unix(entry.file_time),
                    encrypted: entry.is_encrypted(),
                });
                archive = header.skip().map_err(|e| rar_error(e, password.is_some()))?;
            }
        }
        Format::Gz | Format::Bz2 | Format::Xz | Format::Zst => {
            entries.push(Entry { path: stem_name(path), size: 0, is_dir: false, mtime: 0, encrypted: false });
        }
        _ => {
            let mut archive = tar::Archive::new(decoder(path, format)?);
            for entry in archive.entries().context("TAR")? {
                let entry = entry.context("TAR")?;
                let kind = entry.header().entry_type();
                if !kind.is_file() && !kind.is_dir() {
                    continue;
                }
                entries.push(Entry {
                    path: clean(&entry.path().context("TAR")?.to_string_lossy()),
                    size: entry.header().size().unwrap_or(0),
                    is_dir: kind.is_dir(),
                    mtime: entry.header().mtime().unwrap_or(0) as i64 * 1_000_000_000,
                    encrypted: false,
                });
            }
        }
    }
    entries.retain(|e| !e.path.is_empty());
    Ok(entries)
}

struct Plan<'a> {
    dest: Option<&'a Path>,
    select: Option<&'a (dyn Fn(&str) -> bool + Sync)>,
    overwrite: Overwrite,
    strip: usize,
}

impl Plan<'_> {
    fn selected(&self, name: &str) -> bool {
        self.select.is_none_or(|select| select(name))
    }

    fn target(&self, name: &str) -> Result<Option<PathBuf>> {
        let Some(dest) = self.dest else { return Ok(None) };
        let target = safe_target(dest, name, self.strip)?;
        if target.exists() {
            match self.overwrite {
                Overwrite::Skip => return Ok(None),
                Overwrite::Fail => bail!("existiert schon"),
                Overwrite::Replace => {}
            }
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Some(target))
    }

    fn make_dir(&self, name: &str) {
        if let Some(dest) = self.dest {
            if let Ok(target) = safe_target(dest, name, self.strip) {
                let _ = std::fs::create_dir_all(target);
            }
        }
    }
}

fn set_mtime(target: &Path, mtime: i64) {
    if mtime != 0 {
        let time = filetime::FileTime::from_unix_time(mtime.div_euclid(1_000_000_000), mtime.rem_euclid(1_000_000_000) as u32);
        let _ = filetime::set_file_mtime(target, time);
    }
}

fn drain(reader: &mut dyn Read, target: Option<&Path>, progress: &Progress) -> Result<()> {
    let mut out = match target {
        Some(path) => Some(File::create(path)?),
        None => None,
    };
    let mut buf = vec![0u8; 1 << 16];
    loop {
        progress.check()?;
        let got = reader.read(&mut buf)?;
        if got == 0 {
            return Ok(());
        }
        if let Some(out) = out.as_mut() {
            out.write_all(&buf[..got])?;
        }
        progress.add(got as u64);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn extract(
    path: &Path, format: Format, password: Option<&str>, dest: Option<&Path>,
    select: Option<&(dyn Fn(&str) -> bool + Sync)>, overwrite: Overwrite, strip: usize, progress: &Progress,
) -> Result<Vec<String>> {
    let plan = Plan { dest, select, overwrite, strip };
    let listing = list(path, format, password)?;
    progress.set_total(listing.iter().filter(|e| plan.selected(&e.path)).map(|e| e.size).sum());
    if let Some(dest) = dest {
        std::fs::create_dir_all(dest)?;
    }
    let mut failed = Vec::new();
    match format {
        Format::Zip => {
            let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path)?)).context("ZIP")?;
            for (i, entry) in listing.iter().enumerate().take(archive.len()) {
                progress.check()?;
                if !plan.selected(&entry.path) {
                    continue;
                }
                if entry.is_dir {
                    plan.make_dir(&entry.path);
                    continue;
                }
                if entry.encrypted && password.is_none() {
                    bail!(PasswordNeeded);
                }
                let mut attempt = || -> Result<()> {
                    let mut file = match password {
                        Some(password) if entry.encrypted => archive.by_index_decrypt(i, password.as_bytes()),
                        _ => archive.by_index(i),
                    }
                    .map_err(|e| anyhow!("{e}"))?;
                    match (plan.dest, plan.target(&entry.path)?) {
                        (Some(_), None) => Ok(()),
                        (_, target) => {
                            drain(&mut file, target.as_deref(), progress)?;
                            if let Some(target) = target {
                                set_mtime(&target, entry.mtime);
                            }
                            Ok(())
                        }
                    }
                };
                if let Err(e) = attempt() {
                    let text = format!("{e:#}");
                    if text == "abgebrochen" {
                        bail!("abgebrochen");
                    }
                    if entry.encrypted && is_password_error(&text) {
                        bail!("falsches Passwort");
                    }
                    failed.push(format!("{}: {text}", entry.path));
                }
            }
        }
        Format::SevenZ => {
            let mut reader = sevenz_open(path, password)?;
            let result = reader.for_each_entries(|entry, stream| {
                let name = clean(entry.name());
                if name.is_empty() || !plan.selected(&name) || progress.check().is_err() {
                    return Ok(true);
                }
                if entry.is_directory() {
                    plan.make_dir(&name);
                    return Ok(true);
                }
                let mut attempt = || -> Result<()> {
                    match (plan.dest, plan.target(&name)?) {
                        (Some(_), None) => Ok(()),
                        (_, target) => {
                            drain(stream, target.as_deref(), progress)?;
                            if let (Some(target), true) = (target, entry.has_last_modified_date) {
                                set_mtime(&target, system_time_nanos(std::time::SystemTime::from(entry.last_modified_date)));
                            }
                            Ok(())
                        }
                    }
                };
                if let Err(e) = attempt() {
                    failed.push(format!("{name}: {e:#}"));
                }
                Ok(true)
            });
            progress.check()?;
            if let Err(e) = result {
                let text = format!("{e}");
                if is_password_error(&text) {
                    bail!("falsches Passwort");
                }
                failed.push(format!("7z: {text}"));
            }
        }
        Format::Rar => {
            let wrap = |e: unrar::error::UnrarError| rar_error(e, password.is_some());
            let mut archive = rar_open(path, password).open_for_processing().map_err(wrap)?;
            loop {
                progress.check()?;
                let Some(header) = archive.read_header().map_err(wrap)? else { break };
                let entry = header.entry();
                let name = clean(&entry.filename.to_string_lossy());
                let (size, is_dir) = (entry.unpacked_size, entry.is_directory());
                if name.is_empty() || !plan.selected(&name) {
                    archive = header.skip().map_err(wrap)?;
                    continue;
                }
                if is_dir {
                    plan.make_dir(&name);
                    archive = header.skip().map_err(wrap)?;
                    continue;
                }
                let step = match plan.dest {
                    None => header.test(),
                    Some(_) => match plan.target(&name) {
                        Ok(Some(target)) => header.extract_to(target),
                        Ok(None) => header.skip(),
                        Err(e) => {
                            failed.push(format!("{name}: {e:#}"));
                            header.skip()
                        }
                    },
                };
                match step {
                    Ok(next) => archive = next,
                    Err(e) => {
                        let text = e.to_string();
                        if is_password_error(&text) {
                            return Err(rar_error(text, password.is_some()));
                        }
                        failed.push(format!("{name}: {text}"));
                        break;
                    }
                }
                progress.add(size);
            }
        }
        Format::Gz | Format::Bz2 | Format::Xz | Format::Zst => {
            let name = stem_name(path);
            if plan.selected(&name) {
                let attempt = || -> Result<()> {
                    match (plan.dest, plan.target(&name)?) {
                        (Some(_), None) => Ok(()),
                        (_, target) => drain(&mut decoder(path, format)?, target.as_deref(), progress),
                    }
                };
                if let Err(e) = attempt() {
                    failed.push(format!("{name}: {e:#}"));
                }
            }
        }
        _ => {
            debug_assert!(is_tar(format));
            let mut archive = tar::Archive::new(decoder(path, format)?);
            for entry in archive.entries().context("TAR")? {
                progress.check()?;
                let mut entry = entry.context("TAR")?;
                let kind = entry.header().entry_type();
                let name = clean(&entry.path().context("TAR")?.to_string_lossy());
                if name.is_empty() || !plan.selected(&name) || (!kind.is_file() && !kind.is_dir()) {
                    continue;
                }
                if kind.is_dir() {
                    plan.make_dir(&name);
                    continue;
                }
                let mtime = entry.header().mtime().unwrap_or(0) as i64 * 1_000_000_000;
                let mut attempt = || -> Result<()> {
                    match (plan.dest, plan.target(&name)?) {
                        (Some(_), None) => Ok(()),
                        (_, target) => {
                            drain(&mut entry, target.as_deref(), progress)?;
                            if let Some(target) = target {
                                set_mtime(&target, mtime);
                            }
                            Ok(())
                        }
                    }
                };
                if let Err(e) = attempt() {
                    failed.push(format!("{name}: {e:#}"));
                }
            }
        }
    }
    progress.check()?;
    Ok(failed)
}
