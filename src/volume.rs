use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub fn part_name(base: &Path, number: u32) -> PathBuf {
    let stem = base.file_stem().unwrap_or_default().to_string_lossy();
    let ext = base.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "qz".into());
    base.with_file_name(format!("{stem}.part{number:03}.{ext}"))
}

pub fn parts_of(path: &Path) -> Option<(PathBuf, Vec<PathBuf>)> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (rest, ext) = name.rsplit_once('.')?;
    let (stem, part) = rest.rsplit_once('.')?;
    let digits = part.strip_prefix("part").or_else(|| part.strip_prefix("PART"))?;
    if digits.len() < 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let base = path.with_file_name(format!("{stem}.{ext}"));
    let mut parts = Vec::new();
    for number in 1.. {
        let candidate = part_name(&base, number);
        if !candidate.is_file() {
            break;
        }
        parts.push(candidate);
    }
    (!parts.is_empty()).then_some((base, parts))
}

pub struct VolumeWriter {
    base: PathBuf,
    limit: u64,
    current: Option<BufWriter<File>>,
    in_part: u64,
    pub paths: Vec<PathBuf>,
}

impl VolumeWriter {
    pub fn new(base: &Path, limit: u64) -> VolumeWriter {
        VolumeWriter { base: base.to_path_buf(), limit: limit.max(1), current: None, in_part: 0, paths: Vec::new() }
    }

    fn next_part(&mut self) -> io::Result<()> {
        if let Some(mut done) = self.current.take() {
            done.flush()?;
        }
        let path = part_name(&self.base, self.paths.len() as u32 + 1);
        self.current = Some(BufWriter::with_capacity(1 << 20, File::create(&path)?));
        self.paths.push(path);
        self.in_part = 0;
        Ok(())
    }
}

impl Write for VolumeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.current.is_none() || self.in_part >= self.limit {
            self.next_part()?;
        }
        let room = (self.limit - self.in_part).min(buf.len() as u64) as usize;
        let written = self.current.as_mut().unwrap().write(&buf[..room])?;
        self.in_part += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.current.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

pub struct VolumeReader {
    parts: Vec<(File, u64, u64)>,
    pos: u64,
    total: u64,
}

impl VolumeReader {
    pub fn open(paths: &[PathBuf]) -> io::Result<VolumeReader> {
        let mut parts = Vec::new();
        let mut total = 0u64;
        for path in paths {
            let file = File::open(path)?;
            let len = file.metadata()?.len();
            parts.push((file, total, len));
            total += len;
        }
        Ok(VolumeReader { parts, pos: 0, total })
    }

    pub fn len(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

impl Read for VolumeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total || buf.is_empty() {
            return Ok(0);
        }
        let index = self.parts.partition_point(|(_, start, len)| start + len <= self.pos);
        let (file, start, len) = &mut self.parts[index];
        let inside = self.pos - *start;
        let room = (*len - inside).min(buf.len() as u64) as usize;
        file.seek(SeekFrom::Start(inside))?;
        let got = file.read(&mut buf[..room])?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl Seek for VolumeReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::End(offset) => self.total as i128 + offset as i128,
            SeekFrom::Current(offset) => self.pos as i128 + offset as i128,
        };
        if target < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}
