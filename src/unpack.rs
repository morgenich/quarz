use crate::codec;
use crate::crypto::{self, Key};
use crate::format::{FOOTER_LEN, FileEntry, Footer, Header, Index, Insert, Kind, MAGIC, NO_NODE, Node};
use crate::hash;
use crate::progress::Progress;
use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

struct Cache {
    map: HashMap<u32, Arc<Vec<u8>>>,
    order: VecDeque<u32>,
    bytes: u64,
    limit: u64,
}

pub struct Archive {
    io: Mutex<Box<dyn ReadSeek>>,
    pub body_start: u64,
    pub body_len: u64,
    pub header: Header,
    pub header_len: u64,
    pub index: Index,
    pub index_offset: u64,
    pub key: Option<Key>,
    pub volumes: usize,
    pub recovery_percent: u8,
    pub recovery_len: u64,
    pub file_len: u64,
    cache: Mutex<Cache>,
    block_locks: Vec<Mutex<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overwrite {
    Replace,
    Skip,
    Fail,
}

pub struct ExtractOptions<'a> {
    pub dest: Option<&'a Path>,
    pub select: Option<&'a (dyn Fn(&str) -> bool + Sync)>,
    pub overwrite: Overwrite,
    pub strip: usize,
}

fn read_at(io: &mut dyn ReadSeek, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    io.seek(SeekFrom::Start(offset))?;
    io.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn needs_password(io: &mut dyn ReadSeek, body_end: u64) -> Result<bool> {
    let (header, ..) = read_header(io, body_end)?;
    Ok(header.crypt.is_some())
}

fn read_header(io: &mut dyn ReadSeek, body_end: u64) -> Result<(Header, u64, u64, Footer)> {
    if body_end < (FOOTER_LEN + 8) as u64 {
        bail!("kein Quarz-Archiv");
    }
    let footer = Footer::decode(&read_at(io, body_end - FOOTER_LEN as u64, FOOTER_LEN)?)?;
    if footer.body_len > body_end || footer.index_offset + footer.index_len + FOOTER_LEN as u64 != footer.body_len {
        bail!("Archiv ist beschaedigt (Laengen passen nicht)");
    }
    let body_start = body_end - footer.body_len;
    let start = read_at(io, body_start, 8)?;
    if &start[..4] != MAGIC {
        bail!("Archiv ist beschaedigt (Anfangsmarke fehlt)");
    }
    let header_body = u32::from_le_bytes(start[4..8].try_into().unwrap()) as usize;
    if header_body as u64 > footer.body_len {
        bail!("Archiv ist beschaedigt (Kopf)");
    }
    let header: Header = postcard::from_bytes(&read_at(io, body_start + 8, header_body)?).context("Archivkopf")?;
    Ok((header, body_start, 8 + header_body as u64, footer))
}

impl Archive {
    pub fn open(mut io: Box<dyn ReadSeek>, body_end: u64, password: Option<&str>) -> Result<Archive> {
        let (header, body_start, header_len, footer) = read_header(io.as_mut(), body_end)?;
        let key = match (&header.crypt, password) {
            (Some(crypt), Some(password)) => Some(crypto::unlock(password, crypt)?),
            (Some(_), None) => bail!(crate::PasswordNeeded),
            (None, _) => None,
        };
        let stored = read_at(io.as_mut(), body_start + footer.index_offset, footer.index_len as usize)?;
        if hash(&stored) != footer.index_hash {
            bail!("Archiv ist beschaedigt (Inhaltsverzeichnis)");
        }
        let packed = match &key {
            Some(key) => key.open(&stored)?,
            None => stored,
        };
        if packed.len() < 8 {
            bail!("Archiv ist beschaedigt (Inhaltsverzeichnis)");
        }
        let raw_len = u64::from_le_bytes(packed[..8].try_into().unwrap()) as usize;
        let raw = zstd::bulk::decompress(&packed[8..], raw_len).context("Inhaltsverzeichnis")?;
        let index: Index = postcard::from_bytes(&raw).context("Inhaltsverzeichnis")?;
        let block_locks = index.blocks.iter().map(|_| Mutex::new(())).collect();
        Ok(Archive {
            io: Mutex::new(io),
            body_start,
            body_len: footer.body_len,
            header,
            header_len,
            index,
            index_offset: footer.index_offset,
            key,
            volumes: 1,
            recovery_percent: 0,
            recovery_len: 0,
            file_len: body_end,
            cache: Mutex::new(Cache {
                map: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
                limit: (crate::total_ram() / 4).min(8 << 30),
            }),
            block_locks,
        })
    }

    fn block(&self, bid: u32) -> Result<Arc<Vec<u8>>> {
        let info = self.index.blocks.get(bid as usize).ok_or_else(|| anyhow!("Block {bid} fehlt"))?;
        let _loading = self.block_locks[bid as usize].lock().unwrap();
        if let Some(data) = self.cache.lock().unwrap().map.get(&bid) {
            return Ok(data.clone());
        }
        let stored = {
            let mut io = self.io.lock().unwrap();
            read_at(io.as_mut(), self.body_start + info.offset, info.stored as usize)
                .with_context(|| format!("Block {bid} lesen"))?
        };
        let packed = match &self.key {
            Some(key) => key.open(&stored)?,
            None => stored,
        };
        let data = Arc::new(codec::decompress(info.codec, &packed, info.raw as usize).with_context(|| format!("Block {bid}"))?);
        let mut cache = self.cache.lock().unwrap();
        cache.bytes += data.len() as u64;
        cache.map.insert(bid, data.clone());
        cache.order.push_back(bid);
        while cache.bytes > cache.limit && cache.order.len() > 1 {
            let old = cache.order.pop_front().unwrap();
            if let Some(gone) = cache.map.remove(&old) {
                cache.bytes -= gone.len() as u64;
            }
        }
        Ok(data)
    }

    fn rewrap(&self, insert: &Insert) -> Result<Vec<u8>> {
        let child = self.build(insert.child)?;
        let recreate = |plain: &[u8]| -> Result<Vec<u8>> {
            if insert.aux == NO_NODE {
                bail!("Korrekturdaten fehlen");
            }
            let corrections = self.build(insert.aux)?;
            preflate_rs::recreate_whole_deflate_stream(plain, &corrections)
                .map_err(|e| anyhow!("Deflate-Strom nicht wiederherstellbar: {e:?}"))
        };
        let zlib = |plain: &[u8], settings: crate::zre::Zlib| -> Result<Vec<u8>> {
            crate::zre::deflate(plain, settings).ok_or_else(|| anyhow!("Deflate-Strom nicht wiederherstellbar"))
        };
        match &insert.kind {
            Kind::Deflate => recreate(&child),
            Kind::DeflateZlib(settings) => zlib(&child, *settings),
            Kind::Png { prefix, suffix, sizes } => png_chunks(prefix, &recreate(&child)?, suffix, sizes),
            Kind::PngZlib { prefix, suffix, sizes, zlib: settings } => png_chunks(prefix, &zlib(&child, *settings)?, suffix, sizes),
            Kind::Jpeg => {
                let mut out = Vec::with_capacity(child.len() * 5 / 4);
                let features = lepton_jpeg::EnabledFeatures::compat_lepton_vector_read();
                let pool = lepton_jpeg::SingleThreadPool::default();
                lepton_jpeg::decode_lepton(&mut Cursor::new(&child), &mut out, &features, &pool)
                    .map_err(|e| anyhow!("JPEG nicht wiederherstellbar: {e:?}"))?;
                Ok(out)
            }
        }
    }

    pub fn build(&self, nid: u32) -> Result<Vec<u8>> {
        match self.index.nodes.get(nid as usize).ok_or_else(|| anyhow!("Knoten {nid} fehlt"))? {
            Node::Leaf(extents) => {
                let mut out = Vec::with_capacity(extents.iter().map(|e| e.len as usize).sum());
                for extent in extents {
                    let block = self.block(extent.block)?;
                    let (start, end) = (extent.offset as usize, (extent.offset + extent.len) as usize);
                    out.extend_from_slice(block.get(start..end).ok_or_else(|| anyhow!("Bereich ausserhalb des Blocks"))?);
                }
                Ok(out)
            }
            Node::Composite { skeleton, size, inserts } => {
                let skeleton = self.build(*skeleton)?;
                let parts: Vec<Vec<u8>> = if inserts.len() > 1 {
                    inserts.par_iter().map(|i| self.rewrap(i)).collect::<Result<_>>()?
                } else {
                    inserts.iter().map(|i| self.rewrap(i)).collect::<Result<_>>()?
                };
                let mut out = Vec::with_capacity(*size as usize);
                let mut pos = 0usize;
                for (insert, part) in inserts.iter().zip(parts) {
                    let end = pos + insert.gap as usize;
                    out.extend_from_slice(skeleton.get(pos..end).ok_or_else(|| anyhow!("Geruest zu kurz"))?);
                    pos = end;
                    out.extend_from_slice(&part);
                }
                out.extend_from_slice(&skeleton[pos..]);
                if out.len() as u64 != *size {
                    bail!("Knoten {nid} hat die falsche Laenge");
                }
                Ok(out)
            }
        }
    }

    pub fn read_file(&self, entry: &FileEntry) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.size as usize);
        for &part in &entry.parts {
            out.extend_from_slice(&self.build(part)?);
        }
        if hash(&out) != entry.hash {
            bail!("Pruefsumme falsch");
        }
        Ok(out)
    }

    fn extract_one(&self, entry: &FileEntry, options: &ExtractOptions) -> Result<()> {
        let mut hasher = blake3::Hasher::new();
        let Some(dest) = options.dest else {
            for &part in &entry.parts {
                hasher.update(&self.build(part)?);
            }
            return check_hash(&hasher, entry);
        };
        let target = safe_target(dest, &entry.path, options.strip)?;
        if target.exists() {
            match options.overwrite {
                Overwrite::Skip => return Ok(()),
                Overwrite::Fail => bail!("existiert schon"),
                Overwrite::Replace => clear_readonly(&target),
            }
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        {
            let mut file = std::fs::File::create(&target)?;
            for &part in &entry.parts {
                let data = self.build(part)?;
                hasher.update(&data);
                file.write_all(&data)?;
            }
        }
        check_hash(&hasher, entry)?;
        if entry.mtime != 0 {
            let time = filetime::FileTime::from_unix_time(
                entry.mtime.div_euclid(1_000_000_000),
                entry.mtime.rem_euclid(1_000_000_000) as u32,
            );
            let _ = filetime::set_file_mtime(&target, time);
        }
        set_attributes(&target, entry.attrs);
        Ok(())
    }

    pub fn extract(&self, options: &ExtractOptions, progress: &Progress) -> Result<Vec<String>> {
        let selected: Vec<&FileEntry> =
            self.index.files.iter().filter(|f| options.select.is_none_or(|select| select(&f.path))).collect();
        progress.set_total(selected.iter().map(|f| f.size).sum());
        if let Some(dest) = options.dest {
            std::fs::create_dir_all(dest)?;
            for dir in &self.index.dirs {
                if options.select.is_none_or(|select| select(dir)) {
                    if let Ok(target) = safe_target(dest, dir, options.strip) {
                        let _ = std::fs::create_dir_all(target);
                    }
                }
            }
        }
        let failed: Vec<String> = selected
            .par_iter()
            .filter_map(|entry| {
                if progress.check().is_err() {
                    return Some(format!("{}: abgebrochen", entry.path));
                }
                let result = self.extract_one(entry, options);
                progress.add(entry.size);
                result.err().map(|e| format!("{}: {e:#}", entry.path))
            })
            .collect();
        progress.check()?;
        Ok(failed)
    }
}

fn png_chunks(prefix: &[u8; 2], deflate: &[u8], suffix: &[u8; 4], sizes: &[(u32, u32)]) -> Result<Vec<u8>> {
    let mut stream = Vec::with_capacity(deflate.len() + 6);
    stream.extend_from_slice(prefix);
    stream.extend_from_slice(deflate);
    stream.extend_from_slice(suffix);
    let mut out = Vec::with_capacity(stream.len() + 12 * sizes.len());
    let mut at = 0usize;
    for &(size, count) in sizes {
        for _ in 0..count {
            let chunk = stream.get(at..at + size as usize).ok_or_else(|| anyhow!("PNG-Daten passen nicht"))?;
            at += size as usize;
            let mut crc = crc32fast::Hasher::new();
            crc.update(b"IDAT");
            crc.update(chunk);
            out.extend_from_slice(&size.to_be_bytes());
            out.extend_from_slice(b"IDAT");
            out.extend_from_slice(chunk);
            out.extend_from_slice(&crc.finalize().to_be_bytes());
        }
    }
    if at != stream.len() {
        bail!("PNG-Daten passen nicht");
    }
    Ok(out)
}

fn check_hash(hasher: &blake3::Hasher, entry: &FileEntry) -> Result<()> {
    if hasher.finalize().as_bytes()[..16] != entry.hash {
        bail!("Pruefsumme falsch");
    }
    Ok(())
}

pub fn safe_target(dest: &Path, name: &str, strip: usize) -> Result<PathBuf> {
    let mut target = dest.to_path_buf();
    let mut any = false;
    for part in name.split('/').skip(strip) {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']) {
            bail!("unsicherer Pfad im Archiv: {name}");
        }
        target.push(part);
        any = true;
    }
    if !any {
        bail!("leerer Pfad im Archiv: {name}");
    }
    Ok(target)
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn set_attributes(path: &Path, attrs: u32) {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW,
    };
    let keep = attrs & (FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_SYSTEM);
    if keep & !FILE_ATTRIBUTE_ARCHIVE != 0 {
        unsafe { SetFileAttributesW(wide(path).as_ptr(), keep) };
    }
}

#[cfg(not(windows))]
fn set_attributes(_path: &Path, _attrs: u32) {}

fn clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}
