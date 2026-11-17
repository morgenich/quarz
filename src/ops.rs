use crate::codec;
use crate::crypto;
use crate::foreign;
use crate::format::{FOOTER_LEN, Footer, Header, Index, encode_header};
use crate::pack::{Packer, Sealer, Stats, collect, write_index};
use crate::progress::Progress;
use crate::recovery::{self, Report};
use crate::unpack::{Archive, ExtractOptions, Overwrite, ReadSeek};
use crate::volume::{self, VolumeReader, VolumeWriter};
use anyhow::{Context, Result, anyhow, bail};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct CreateOptions {
    pub level: u8,
    pub threads: usize,
    pub password: Option<String>,
    pub comment: String,
    pub volume_size: Option<u64>,
    pub recovery: u8,
    pub sfx: Option<PathBuf>,
}

impl Default for CreateOptions {
    fn default() -> Self {
        CreateOptions {
            level: 3,
            threads: default_threads(),
            password: None,
            comment: String::new(),
            volume_size: None,
            recovery: 0,
            sfx: None,
        }
    }
}

pub fn default_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

pub fn now_nanos() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

fn part_path(archive: &Path) -> PathBuf {
    let mut name = archive.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

pub fn sealer_for(key: &Option<crypto::Key>) -> Sealer {
    key.clone().map(|key| Arc::new(move |data: &[u8]| key.seal(data)) as Arc<dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync>)
}

pub fn parse_size(text: &str) -> Result<u64> {
    let text = text.trim().to_lowercase();
    let (digits, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit() && *c != '.' && *c != ',') {
        Some((at, _)) => text.split_at(at),
        None => (text.as_str(), ""),
    };
    let number: f64 = digits.replace(',', ".").parse().map_err(|_| anyhow!("ungueltige Groesse: {text}"))?;
    let factor = match unit.trim().trim_end_matches('b') {
        "" => 1.0,
        "k" => 1e3,
        "m" => 1e6,
        "g" => 1e9,
        _ => bail!("ungueltige Einheit in: {text}"),
    };
    let bytes = (number * factor) as u64;
    if bytes < 65536 {
        bail!("Teile muessen mindestens 64 KB gross sein");
    }
    Ok(bytes)
}

pub fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => wildcard(&pattern[1..], text) || (!text.is_empty() && wildcard(pattern, &text[1..])),
        (Some(b'?'), Some(_)) => wildcard(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p.eq_ignore_ascii_case(t) => wildcard(&pattern[1..], &text[1..]),
        _ => false,
    }
}

pub fn selects(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|raw| {
        let pattern = raw.replace('\\', "/");
        let pattern = pattern.trim_end_matches('/');
        let name = path.rsplit('/').next().unwrap_or(path);
        path.eq_ignore_ascii_case(pattern)
            || (path.len() > pattern.len()
                && path.is_char_boundary(pattern.len())
                && path.as_bytes()[pattern.len()] == b'/'
                && path[..pattern.len()].eq_ignore_ascii_case(pattern))
            || wildcard(pattern.as_bytes(), path.as_bytes())
            || wildcard(pattern.as_bytes(), name.as_bytes())
    })
}

pub fn format_size(bytes: u64) -> String {
    let (value, unit) = match bytes {
        0..=999 => return format!("{bytes} B"),
        1_000..=999_999 => (bytes as f64 / 1e3, "KB"),
        1_000_000..=999_999_999 => (bytes as f64 / 1e6, "MB"),
        _ => (bytes as f64 / 1e9, "GB"),
    };
    format!("{value:.1} {unit}").replace('.', ",")
}

pub fn local_time(nanos: i64) -> Option<[u32; 5]> {
    if nanos <= 0 {
        return None;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        let ticks = (nanos / 100 + 116_444_736_000_000_000) as u64;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            FileTimeToSystemTime(&ft, &mut utc) != 0
                && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
        };
        ok.then(|| [local.wYear as u32, local.wMonth as u32, local.wDay as u32, local.wHour as u32, local.wMinute as u32])
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn format_time(nanos: i64) -> String {
    match local_time(nanos) {
        Some([year, month, day, hour, minute]) => format!("{day:02}.{month:02}.{year} {hour:02}:{minute:02}"),
        None => String::new(),
    }
}

enum Output {
    Single { file: BufWriter<File>, part: PathBuf, target: PathBuf },
    Volumes(VolumeWriter),
}

impl Output {
    fn new(archive: &Path, volume_size: Option<u64>) -> Result<Output> {
        Ok(match volume_size {
            Some(size) => Output::Volumes(VolumeWriter::new(archive, size)),
            None => {
                let part = part_path(archive);
                let file = File::create(&part).with_context(|| format!("kann {} nicht anlegen", part.display()))?;
                Output::Single { file: BufWriter::with_capacity(1 << 20, file), part, target: archive.to_path_buf() }
            }
        })
    }

    fn reader(&mut self) -> Result<Box<dyn ReadSeek>> {
        self.flush()?;
        Ok(match self {
            Output::Single { part, .. } => Box::new(File::open(part)?),
            Output::Volumes(writer) => Box::new(VolumeReader::open(&writer.paths)?),
        })
    }

    fn commit(mut self) -> Result<()> {
        self.flush()?;
        if let Output::Single { file, part, target } = self {
            drop(file);
            std::fs::rename(&part, &target).with_context(|| format!("kann {} nicht anlegen", target.display()))?;
        }
        Ok(())
    }

    fn discard(self) {
        match self {
            Output::Single { file, part, .. } => {
                drop(file);
                let _ = std::fs::remove_file(part);
            }
            Output::Volumes(writer) => {
                let paths = writer.paths.clone();
                drop(writer);
                for path in paths {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Output::Single { file, .. } => file.write(buf),
            Output::Volumes(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Output::Single { file, .. } => file.flush(),
            Output::Volumes(writer) => writer.flush(),
        }
    }
}

pub fn create(archive: &Path, inputs: &[PathBuf], options: &CreateOptions, progress: &Progress) -> Result<Stats> {
    let (sources, dirs) = collect(inputs, &[archive.to_path_buf(), part_path(archive)])?;
    if sources.is_empty() && dirs.is_empty() {
        bail!("nichts zu packen");
    }
    if options.sfx.is_some() && options.volume_size.is_some() {
        bail!("selbstentpackende Archive koennen nicht aufgeteilt werden");
    }
    progress.set_total(sources.iter().map(|s| s.size).sum());
    let mut out = Output::new(archive, options.volume_size)?;
    let mut attempt = || -> Result<Stats> {
        let mut stub_len = 0u64;
        if let Some(stub) = &options.sfx {
            let stub = std::fs::read(stub).with_context(|| format!("kann {} nicht lesen", stub.display()))?;
            out.write_all(&stub)?;
            stub_len = stub.len() as u64;
        }
        let (crypt, key) = match &options.password {
            Some(password) => {
                let (header, key) = crypto::create(password)?;
                (Some(header), Some(key))
            }
            None => (None, None),
        };
        let header = encode_header(&Header { version: 2, crypt })?;
        out.write_all(&header)?;
        let mut packer =
            Packer::new(&mut out, header.len() as u64, codec::level(options.level), options.threads, sealer_for(&key), None);
        packer.dirs = dirs.clone();
        packer.progress = Some(progress);
        packer.add_files(&sources, progress)?;
        let mut stats = std::mem::take(&mut packer.stats);
        let index = Index { comment: options.comment.clone(), created: now_nanos(), ..Default::default() };
        let clock = std::time::Instant::now();
        let body_len = packer.finish(index)?;
        stats.finish_s = clock.elapsed().as_secs_f64();
        if options.recovery > 0 {
            let mut io = out.reader()?;
            recovery::append(io.as_mut(), stub_len, body_len, options.recovery, &mut out, progress)?;
        }
        Ok(stats)
    };
    match attempt() {
        Ok(stats) => {
            out.commit()?;
            Ok(stats)
        }
        Err(e) => {
            out.discard();
            Err(e)
        }
    }
}

pub fn open_io(archive: &Path) -> Result<(Box<dyn ReadSeek>, u64, usize)> {
    if let Some((_, parts)) = volume::parts_of(archive) {
        let reader = VolumeReader::open(&parts)?;
        let total = reader.len();
        return Ok((Box::new(reader), total, parts.len()));
    }
    let file = File::open(archive).with_context(|| format!("kann {} nicht oeffnen", archive.display()))?;
    let total = file.metadata()?.len();
    Ok((Box::new(file), total, 1))
}

pub fn open(archive: &Path, password: Option<&str>) -> Result<Archive> {
    let (mut io, total, volumes) = open_io(archive)?;
    let recovery_len = recovery::section_len(io.as_mut(), total)?;
    let recovery_percent = if recovery_len > 0 { recovery::percent(io.as_mut(), total).unwrap_or(0) } else { 0 };
    let mut opened = Archive::open(io, total - recovery_len, password)?;
    opened.volumes = volumes;
    opened.recovery_len = recovery_len;
    opened.recovery_percent = recovery_percent;
    opened.file_len = total;
    Ok(opened)
}

pub fn is_encrypted(archive: &Path) -> Result<bool> {
    let (mut io, total, _) = open_io(archive)?;
    let recovery_len = recovery::section_len(io.as_mut(), total)?;
    crate::unpack::needs_password(io.as_mut(), total - recovery_len)
}

pub fn is_quarz(path: &Path) -> bool {
    let attempt = || -> Result<bool> {
        let (mut io, total, _) = open_io(path)?;
        let recovery_len = recovery::section_len(io.as_mut(), total)?;
        let end = total - recovery_len;
        if end < FOOTER_LEN as u64 {
            return Ok(false);
        }
        let mut raw = [0u8; FOOTER_LEN];
        io.seek(SeekFrom::Start(end - FOOTER_LEN as u64))?;
        io.read_exact(&mut raw)?;
        Ok(Footer::decode(&raw).is_ok())
    };
    attempt().unwrap_or(false)
}

pub fn embedded_archive() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    is_quarz(&exe).then_some(exe)
}

fn modify<T>(
    archive: &Path, password: Option<&str>, progress: &Progress,
    change: impl FnOnce(&Archive, &mut dyn Write, &Sealer) -> Result<(u64, T)>,
) -> Result<T> {
    if volume::parts_of(archive).is_some() {
        bail!("mehrteilige Archive koennen nicht geaendert werden");
    }
    let source = open(archive, password)?;
    if source.index.locked {
        bail!("das Archiv ist gesperrt und kann nicht geaendert werden");
    }
    let part = part_path(archive);
    let attempt = || -> Result<T> {
        let keep = source.body_start + source.index_offset;
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&part)?);
        std::io::copy(&mut File::open(archive)?.take(keep), &mut out)?;
        let (body_len, result) = change(&source, &mut out, &sealer_for(&source.key))?;
        out.flush()?;
        if source.recovery_percent > 0 {
            let mut io = File::open(&part)?;
            recovery::append(&mut io, source.body_start, body_len, source.recovery_percent, &mut out, progress)?;
        }
        out.flush()?;
        Ok(result)
    };
    let result = attempt();
    drop(source);
    match result {
        Ok(value) => {
            std::fs::rename(&part, archive).with_context(|| format!("kann {} nicht ersetzen", archive.display()))?;
            Ok(value)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

pub fn add(
    archive: &Path, inputs: &[PathBuf], password: Option<&str>, level: Option<u8>, threads: usize, progress: &Progress,
) -> Result<Stats> {
    let (sources, dirs) = collect(inputs, &[archive.to_path_buf(), part_path(archive)])?;
    progress.set_total(sources.iter().map(|s| s.size).sum());
    modify(archive, password, progress, |source, out, sealer| {
        let level = codec::level(level.unwrap_or(source.index.level));
        let mut packer =
            Packer::new(out, source.index_offset, level, threads, sealer.clone(), Some(source.index.clone()));
        packer.progress = Some(progress);
        for dir in &dirs {
            if !packer.dirs.iter().any(|d| d.eq_ignore_ascii_case(dir)) {
                packer.dirs.push(dir.clone());
            }
        }
        packer.add_files(&sources, progress)?;
        let stats = std::mem::take(&mut packer.stats);
        let meta = Index { comment: source.index.comment.clone(), created: source.index.created, ..Default::default() };
        Ok((packer.finish(meta)?, stats))
    })
}

fn edit_index(
    archive: &Path, password: Option<&str>, progress: &Progress, edit: impl FnOnce(&mut Index) -> Result<()>,
) -> Result<()> {
    modify(archive, password, progress, |source, out, sealer| {
        let mut index = source.index.clone();
        edit(&mut index)?;
        Ok((write_index(out, source.index_offset, &index, sealer)?, ()))
    })
}

pub fn delete(archive: &Path, patterns: &[String], password: Option<&str>, progress: &Progress) -> Result<usize> {
    let mut removed = 0usize;
    edit_index(archive, password, progress, |index| {
        let before = index.files.len() + index.dirs.len();
        removed = index.files.len();
        index.files.retain(|f| !selects(patterns, &f.path));
        removed -= index.files.len();
        index.dirs.retain(|d| !selects(patterns, d));
        if before == index.files.len() + index.dirs.len() {
            bail!("nichts im Archiv passt auf die Auswahl");
        }
        Ok(())
    })?;
    Ok(removed)
}

pub fn set_comment(archive: &Path, comment: &str, password: Option<&str>, progress: &Progress) -> Result<()> {
    edit_index(archive, password, progress, |index| {
        index.comment = comment.to_string();
        Ok(())
    })
}

pub fn lock(archive: &Path, password: Option<&str>, progress: &Progress) -> Result<()> {
    edit_index(archive, password, progress, |index| {
        index.locked = true;
        Ok(())
    })
}

pub fn protect(archive: &Path, percent: u8, progress: &Progress) -> Result<()> {
    if volume::parts_of(archive).is_some() {
        bail!("mehrteilige Archive bitte gleich beim Anlegen mit Wiederherstellungsdaten versehen");
    }
    let (mut io, total, _) = open_io(archive)?;
    let old = recovery::section_len(io.as_mut(), total)?;
    let end = total - old;
    let mut raw = [0u8; FOOTER_LEN];
    io.seek(SeekFrom::Start(end.checked_sub(FOOTER_LEN as u64).ok_or_else(|| anyhow!("kein Quarz-Archiv"))?))?;
    io.read_exact(&mut raw)?;
    let footer = Footer::decode(&raw)?;
    let body_start = end.checked_sub(footer.body_len).ok_or_else(|| anyhow!("Archiv ist beschaedigt"))?;
    let part = part_path(archive);
    let mut attempt = || -> Result<()> {
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&part)?);
        std::io::copy(&mut File::open(archive)?.take(end), &mut out)?;
        recovery::append(io.as_mut(), body_start, footer.body_len, percent, &mut out, progress)?;
        out.flush()?;
        Ok(())
    };
    let result = attempt();
    drop(io);
    match result {
        Ok(()) => Ok(std::fs::rename(&part, archive)?),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

pub fn check_recovery(archive: &Path, progress: &Progress) -> Result<Option<Report>> {
    let (mut io, total, _) = open_io(archive)?;
    recovery::check(io.as_mut(), total, progress)
}

pub fn repair(archive: &Path, output: &Path, progress: &Progress) -> Result<Report> {
    let (mut io, total, _) = open_io(archive)?;
    let mut attempt = || -> Result<Report> {
        io.seek(SeekFrom::Start(0))?;
        let mut out = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(output)?;
        std::io::copy(&mut io, &mut out)?;
        recovery::repair(io.as_mut(), total, &mut out, progress)
    };
    let result = attempt();
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

pub fn join(part: &Path, output: &Path) -> Result<usize> {
    let Some((_, parts)) = volume::parts_of(part) else {
        bail!("{} ist kein Teil eines aufgeteilten Archivs (erwartet: name.part001.qz)", part.display());
    };
    if parts.iter().any(|p| p == output) {
        bail!("die Ausgabe darf keiner der Teile sein");
    }
    let attempt = || -> Result<()> {
        let mut out = BufWriter::with_capacity(1 << 20, File::create(output)?);
        for path in &parts {
            std::io::copy(&mut File::open(path)?, &mut out)?;
        }
        out.flush()?;
        if !is_quarz(output) {
            bail!("die Teile ergeben kein vollstaendiges Archiv (fehlt ein Teil?)");
        }
        Ok(())
    };
    if let Err(e) = attempt() {
        let _ = std::fs::remove_file(output);
        return Err(e);
    }
    Ok(parts.len())
}

pub struct Listing {
    pub kind: String,
    pub entries: Vec<foreign::Entry>,
    pub notes: Vec<String>,
    pub comment: String,
    pub packed: u64,
    pub editable: bool,
    pub encrypted: bool,
    pub parts: usize,
    pub protection: u8,
    pub self_extracting: bool,
    pub locked: bool,
    pub level: Option<u8>,
}

pub fn listing(path: &Path, password: Option<&str>) -> Result<Listing> {
    if is_quarz(path) {
        let archive = open(path, password)?;
        let mut entries: Vec<foreign::Entry> = archive
            .index
            .files
            .iter()
            .map(|f| foreign::Entry { path: f.path.clone(), size: f.size, is_dir: false, mtime: f.mtime, encrypted: false })
            .collect();
        entries.extend(archive.index.dirs.iter().map(|d| foreign::Entry {
            path: d.clone(),
            size: 0,
            is_dir: true,
            mtime: 0,
            encrypted: false,
        }));
        let mut notes = vec![format!("Stufe {}", codec::level(archive.index.level).name)];
        if archive.header.crypt.is_some() {
            notes.push("verschluesselt".into());
        }
        if archive.volumes > 1 {
            notes.push(format!("{} Teile", archive.volumes));
        }
        if archive.recovery_percent > 0 {
            notes.push(format!("Wiederherstellung {} %", archive.recovery_percent));
        }
        if archive.body_start > 0 {
            notes.push("selbstentpackend".into());
        }
        if archive.index.locked {
            notes.push("gesperrt".into());
        }
        return Ok(Listing {
            kind: "Quarz".into(),
            entries,
            notes,
            comment: archive.index.comment.clone(),
            packed: archive.file_len,
            editable: archive.volumes == 1 && !archive.index.locked,
            encrypted: archive.header.crypt.is_some(),
            parts: archive.volumes,
            protection: archive.recovery_percent,
            self_extracting: archive.body_start > 0,
            locked: archive.index.locked,
            level: Some(archive.index.level),
        });
    }
    let format = foreign::detect(path).ok_or_else(|| anyhow!("{} ist kein bekanntes Archiv", path.display()))?;
    let entries = foreign::list(path, format, password)?;
    let mut notes = Vec::new();
    let encrypted = entries.iter().any(|e| e.encrypted);
    if encrypted {
        notes.push("verschluesselt".into());
    }
    Ok(Listing {
        kind: format.name().into(),
        entries,
        notes,
        comment: String::new(),
        packed: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        editable: false,
        encrypted,
        parts: 1,
        protection: 0,
        self_extracting: false,
        locked: false,
        level: None,
    })
}

pub fn extract_any(
    path: &Path, password: Option<&str>, dest: Option<&Path>, patterns: &[String], overwrite: Overwrite, strip: usize,
    progress: &Progress,
) -> Result<Vec<String>> {
    let select = |name: &str| selects(patterns, name);
    let select: Option<&(dyn Fn(&str) -> bool + Sync)> = if patterns.is_empty() { None } else { Some(&select) };
    if is_quarz(path) {
        let archive = open(path, password)?;
        return archive.extract(&ExtractOptions { dest, select, overwrite, strip }, progress);
    }
    let format = foreign::detect(path).ok_or_else(|| anyhow!("{} ist kein bekanntes Archiv", path.display()))?;
    foreign::extract(path, format, password, dest, select, overwrite, strip, progress)
}

pub fn is_archive(path: &Path) -> bool {
    path.is_file() && (is_quarz(path) || foreign::detect(path).is_some())
}

pub fn make_sfx(archive: &Path, stub: &Path, output: &Path) -> Result<()> {
    if volume::parts_of(archive).is_some() {
        bail!("mehrteilige Archive koennen nicht selbstentpackend gemacht werden");
    }
    if !is_quarz(archive) {
        bail!("{} ist kein Quarz-Archiv", archive.display());
    }
    let mut out = BufWriter::with_capacity(1 << 20, File::create(output)?);
    std::io::copy(&mut File::open(stub).with_context(|| format!("kann {} nicht lesen", stub.display()))?, &mut out)?;
    std::io::copy(&mut File::open(archive)?, &mut out)?;
    out.flush()?;
    Ok(())
}
