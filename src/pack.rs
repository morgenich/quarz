use crate::classify::{Class, classify};
use crate::codec::{self, Level};
use crate::format::{BlockInfo, Codec, Extent, FileEntry, Footer, Index, Insert, NO_NODE, Node};
use crate::progress::Progress;
use crate::scan::{Ctx, Piece, Tree, unwrap};
use crate::{Hash, hash};
use anyhow::{Context, Result, anyhow, bail};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use fastcdc::v2020::FastCDC;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const CHUNK_MIN: usize = 2048;
const CHUNK_AVG: usize = 8192;
const CHUNK_MAX: usize = 65536;
const CHUNKED_FROM: usize = 32768;
const SEGMENT: usize = 1 << 20;
const BIG_FILE: u64 = 1 << 30;
const PIECE: usize = 64 << 20;
const BATCH_BYTES: u64 = 256 << 20;

#[derive(Clone, Debug)]
pub struct Source {
    pub arc: String,
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub files: u64,
    pub bytes_in: u64,
    pub unwrapped: u64,
    pub unwrapped_bytes: u64,
    pub dedup_bytes: u64,
    pub skipped: Vec<String>,
    pub load_s: f64,
    pub intern_s: f64,
    pub finish_s: f64,
}

pub type Sealer = Option<std::sync::Arc<dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync>>;

struct Job {
    bid: u32,
    class: Class,
    data: Vec<u8>,
}

struct Done {
    bid: u32,
    raw: u64,
    result: Result<(Codec, Vec<u8>)>,
}

pub struct Packer<'a> {
    level: Level,
    out: &'a mut dyn Write,
    pos: u64,
    sealer: Sealer,
    blocks: Vec<Option<BlockInfo>>,
    nodes: Vec<Node>,
    by_hash: HashMap<Hash, u32>,
    chunks: HashMap<Hash, (u32, u64)>,
    current: HashMap<Class, (u32, Vec<u8>)>,
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    in_flight: usize,
    pub files: Vec<FileEntry>,
    pub dirs: Vec<String>,
    pub stats: Stats,
    pub progress: Option<&'a Progress>,
}

fn file_times(meta: &std::fs::Metadata) -> (i64, u32) {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    #[cfg(windows)]
    let attrs = std::os::windows::fs::MetadataExt::file_attributes(meta);
    #[cfg(not(windows))]
    let attrs = 0;
    (mtime, attrs)
}

enum Loaded {
    Tree(Tree, i64, u32),
    Big,
    Failed(String),
}

impl<'a> Packer<'a> {
    pub fn new(
        out: &'a mut dyn Write, pos: u64, level: Level, threads: usize, sealer: Sealer, existing: Option<Index>,
    ) -> Packer<'a> {
        let budget = crate::total_ram() / 2;
        let workers = (budget / level.worker_memory().max(1)).clamp(1, threads.max(1) as u64) as usize;
        let (jobs, job_rx) = bounded::<Job>(workers);
        let (done_tx, done) = unbounded::<Done>();
        let trace = std::env::var_os("QZ_TRACE").is_some();
        if trace {
            eprintln!("Kompressor-Threads: {workers}");
        }
        for _ in 0..workers {
            let (rx, tx, sealer) = (job_rx.clone(), done_tx.clone(), sealer.clone());
            std::thread::spawn(move || {
                for job in rx {
                    let raw = job.data.len() as u64;
                    let clock = std::time::Instant::now();
                    let result = codec::compress(job.class, &job.data, &level).and_then(|(codec, data)| match &sealer {
                        Some(seal) => Ok((codec, seal(&data)?)),
                        None => Ok((codec, data)),
                    });
                    if trace {
                        let packed = result.as_ref().map(|r| r.1.len()).unwrap_or(0);
                        eprintln!(
                            "Block {:3} {:?}: {:6.1} MB -> {:6.1} MB in {:5.1} s",
                            job.bid, job.class, raw as f64 / 1e6, packed as f64 / 1e6, clock.elapsed().as_secs_f64()
                        );
                    }
                    if tx.send(Done { bid: job.bid, raw, result }).is_err() {
                        break;
                    }
                }
            });
        }
        let existing = existing.unwrap_or_default();
        let by_hash: HashMap<Hash, u32> =
            existing.files.iter().filter(|f| f.parts.len() == 1).map(|f| (f.hash, f.parts[0])).collect();
        Packer {
            level,
            out,
            pos,
            sealer,
            blocks: existing.blocks.into_iter().map(Some).collect(),
            nodes: existing.nodes,
            by_hash,
            chunks: HashMap::new(),
            current: HashMap::new(),
            jobs: Some(jobs),
            done,
            in_flight: 0,
            files: existing.files,
            dirs: existing.dirs,
            stats: Stats::default(),
            progress: None,
        }
    }

    fn write_done(&mut self, done: Done) -> Result<()> {
        let (codec, data) = done.result?;
        self.out.write_all(&data).context("Schreiben des Archivs")?;
        self.blocks[done.bid as usize] =
            Some(BlockInfo { offset: self.pos, stored: data.len() as u64, raw: done.raw, codec });
        self.pos += data.len() as u64;
        self.in_flight -= 1;
        if let Some(progress) = self.progress {
            progress.stage_done.fetch_add(done.raw, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    fn flush(&mut self, class: Class) -> Result<()> {
        let Some((bid, data)) = self.current.remove(&class) else { return Ok(()) };
        self.in_flight += 1;
        if let Some(progress) = self.progress {
            progress.stage_total.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        self.jobs.as_ref().unwrap().send(Job { bid, class, data }).map_err(|_| anyhow!("Kompressor beendet"))?;
        while let Ok(done) = self.done.try_recv() {
            self.write_done(done)?;
        }
        Ok(())
    }

    fn extend(extents: &mut Vec<Extent>, block: u32, offset: u64, len: u64) {
        if let Some(last) = extents.last_mut() {
            if last.block == block && last.offset + last.len == offset {
                last.len += len;
                return;
            }
        }
        extents.push(Extent { block, offset, len });
    }

    fn place(
        &mut self, extents: &mut Vec<Extent>, data: &[u8], class: Option<Class>, fresh: &[(Hash, usize)], base: usize,
    ) -> Result<()> {
        let class = class.unwrap_or_else(|| classify(data, self.level.store_above));
        if !self.current.contains_key(&class) {
            self.blocks.push(None);
            self.current.insert(class, (self.blocks.len() as u32 - 1, Vec::new()));
        }
        let (bid, buf) = self.current.get_mut(&class).unwrap();
        let (bid, offset) = (*bid, buf.len() as u64);
        buf.extend_from_slice(data);
        let full = buf.len() >= self.level.block;
        for (chunk_hash, start) in fresh {
            self.chunks.insert(*chunk_hash, (bid, offset + (start - base) as u64));
        }
        Self::extend(extents, bid, offset, data.len() as u64);
        if full {
            self.flush(class)?;
        }
        Ok(())
    }

    fn add_leaf(&mut self, data: &[u8], class: Option<Class>, h: Hash) -> Result<u32> {
        if let Some(&id) = self.by_hash.get(&h) {
            self.stats.dedup_bytes += data.len() as u64;
            return Ok(id);
        }
        let mut extents = Vec::new();
        if data.is_empty() {
        } else if data.len() < CHUNKED_FROM {
            self.place(&mut extents, data, class, &[], 0)?;
        } else {
            let hint = if data.len() <= SEGMENT { class } else { None };
            let mut fresh: Vec<(Hash, usize)> = Vec::new();
            let mut seg = 0usize;
            for chunk in FastCDC::new(data, CHUNK_MIN, CHUNK_AVG, CHUNK_MAX) {
                let (pos, cut) = (chunk.offset, chunk.offset + chunk.length);
                let chunk_hash = hash(&data[pos..cut]);
                match self.chunks.get(&chunk_hash).copied() {
                    None => {
                        fresh.push((chunk_hash, pos));
                        if cut - seg >= SEGMENT {
                            self.place(&mut extents, &data[seg..cut], hint, &fresh, seg)?;
                            fresh.clear();
                            seg = cut;
                        }
                    }
                    Some((block, offset)) => {
                        if pos > seg {
                            self.place(&mut extents, &data[seg..pos], hint, &fresh, seg)?;
                            fresh.clear();
                        }
                        Self::extend(&mut extents, block, offset, (cut - pos) as u64);
                        self.stats.dedup_bytes += (cut - pos) as u64;
                        seg = cut;
                    }
                }
            }
            if seg < data.len() {
                self.place(&mut extents, &data[seg..], hint, &fresh, seg)?;
            }
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(Node::Leaf(extents));
        self.by_hash.insert(h, id);
        Ok(id)
    }

    fn mostly_known(&self, tree: &Tree) -> bool {
        let Tree::Leaf { hash: h, data, .. } = tree else { return true };
        if self.by_hash.contains_key(h) {
            return true;
        }
        if data.len() < CHUNKED_FROM {
            return false;
        }
        let known: usize = FastCDC::new(data, CHUNK_MIN, CHUNK_AVG, CHUNK_MAX)
            .filter(|c| self.chunks.contains_key(&hash(&data[c.offset..c.offset + c.length])))
            .map(|c| c.length)
            .sum();
        known * 2 >= data.len()
    }

    fn intern(&mut self, tree: Tree) -> Result<u32> {
        if let Some(&id) = self.by_hash.get(&tree.hash()) {
            self.stats.dedup_bytes += tree.size() as u64;
            return Ok(id);
        }
        let (h, data, pieces) = match tree {
            Tree::Leaf { hash, data, class } => return self.add_leaf(&data, class, hash),
            Tree::Comp { hash, data, pieces } => (hash, data, pieces),
        };
        let mut skeleton = Vec::new();
        let mut inserts = Vec::new();
        let mut gap = 0u64;
        for piece in pieces {
            match piece {
                Piece::Raw(range) => {
                    gap += range.len() as u64;
                    skeleton.extend_from_slice(&data[range]);
                }
                Piece::Stream { range, kind, child, corrections, doubtful } => {
                    if doubtful && !self.mostly_known(&child) {
                        gap += range.len() as u64;
                        skeleton.extend_from_slice(&data[range]);
                        continue;
                    }
                    self.stats.unwrapped += 1;
                    self.stats.unwrapped_bytes += child.size() as u64;
                    let child = self.intern(*child)?;
                    let aux = if corrections.is_empty() {
                        NO_NODE
                    } else {
                        self.add_leaf(&corrections, Some(Class::Corr), hash(&corrections))?
                    };
                    inserts.push(Insert { gap, kind, child, aux });
                    gap = 0;
                }
            }
        }
        if inserts.is_empty() {
            return self.add_leaf(&data, None, h);
        }
        let class = (skeleton.len() <= SEGMENT).then_some(Class::Skel);
        let skeleton = self.add_leaf(&skeleton, class, hash(&skeleton))?;
        let id = self.nodes.len() as u32;
        self.nodes.push(Node::Composite { skeleton, size: data.len() as u64, inserts });
        self.by_hash.insert(h, id);
        Ok(id)
    }

    fn add_big(&mut self, source: &Source, progress: &Progress) -> Result<FileEntry> {
        let mut file = std::fs::File::open(&source.path)?;
        let (mtime, attrs) = file_times(&file.metadata()?);
        let mut hasher = blake3::Hasher::new();
        let (mut parts, mut size) = (Vec::new(), 0u64);
        loop {
            progress.check()?;
            let mut piece = Vec::with_capacity(PIECE);
            (&mut file).take(PIECE as u64).read_to_end(&mut piece)?;
            if piece.is_empty() {
                break;
            }
            hasher.update(&piece);
            size += piece.len() as u64;
            parts.push(self.add_leaf(&piece, None, hash(&piece))?);
            progress.add(piece.len() as u64);
        }
        let mut h = [0u8; 16];
        h.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        Ok(FileEntry { path: source.arc.clone(), size, mtime, attrs, hash: h, parts })
    }

    pub fn add_files(&mut self, sources: &[Source], progress: &Progress) -> Result<()> {
        let mut start = 0;
        while start < sources.len() {
            progress.check()?;
            let mut end = start;
            let mut bytes = 0u64;
            while end < sources.len() && (end == start || bytes + sources[end].size <= BATCH_BYTES) {
                bytes += sources[end].size;
                end += 1;
            }
            let batch = &sources[start..end];
            start = end;

            let clock = std::time::Instant::now();
            let loaded: Vec<Loaded> = {
                let ctx = Ctx { known: &self.by_hash, level: &self.level };
                batch
                    .par_iter()
                    .map(|source| {
                        let attempt = || -> Result<Loaded> {
                            let meta = std::fs::metadata(&source.path)?;
                            if meta.len() > BIG_FILE {
                                return Ok(Loaded::Big);
                            }
                            let data = std::fs::read(&source.path)?;
                            let (mtime, attrs) = file_times(&meta);
                            progress.add(data.len() as u64);
                            Ok(Loaded::Tree(unwrap(data, 0, &ctx), mtime, attrs))
                        };
                        attempt().unwrap_or_else(|e| Loaded::Failed(e.to_string()))
                    })
                    .collect()
            };
            self.stats.load_s += clock.elapsed().as_secs_f64();
            let clock = std::time::Instant::now();

            for (source, loaded) in batch.iter().zip(loaded) {
                let entry = match loaded {
                    Loaded::Tree(tree, mtime, attrs) => {
                        let (h, size) = (tree.hash(), tree.size() as u64);
                        let parts = if size == 0 { Vec::new() } else { vec![self.intern(tree)?] };
                        FileEntry { path: source.arc.clone(), size, mtime, attrs, hash: h, parts }
                    }
                    Loaded::Big => match self.add_big(source, progress) {
                        Ok(entry) => entry,
                        Err(e) if e.to_string() == "abgebrochen" => return Err(e),
                        Err(e) => {
                            self.stats.skipped.push(format!("{}: {e}", source.arc));
                            continue;
                        }
                    },
                    Loaded::Failed(reason) => {
                        self.stats.skipped.push(format!("{}: {reason}", source.arc));
                        continue;
                    }
                };
                self.stats.files += 1;
                self.stats.bytes_in += entry.size;
                self.files.retain(|f| !f.path.eq_ignore_ascii_case(&entry.path));
                self.files.push(entry);
            }
            self.stats.intern_s += clock.elapsed().as_secs_f64();
        }
        Ok(())
    }

    pub fn finish(mut self, mut index: Index) -> Result<u64> {
        let classes: Vec<Class> = self.current.keys().copied().collect();
        for class in classes {
            self.flush(class)?;
        }
        self.jobs = None;
        while self.in_flight > 0 {
            let done = self.done.recv().map_err(|_| anyhow!("Kompressor beendet"))?;
            self.write_done(done)?;
        }
        index.level = self.level.id;
        index.blocks = self.blocks.drain(..).map(|b| b.ok_or_else(|| anyhow!("Block fehlt"))).collect::<Result<_>>()?;
        index.nodes = std::mem::take(&mut self.nodes);
        index.files = std::mem::take(&mut self.files);
        index.dirs = std::mem::take(&mut self.dirs);
        write_index(self.out, self.pos, &index, &self.sealer)
    }
}

pub fn write_index(out: &mut dyn Write, pos: u64, index: &Index, sealer: &Sealer) -> Result<u64> {
    let raw = postcard::to_stdvec(index)?;
    let mut packed = (raw.len() as u64).to_le_bytes().to_vec();
    packed.extend_from_slice(&zstd::bulk::compress(&raw, 19)?);
    let stored = match sealer {
        Some(seal) => seal(&packed)?,
        None => packed,
    };
    out.write_all(&stored)?;
    let body_len = pos + stored.len() as u64 + crate::format::FOOTER_LEN as u64;
    let footer = Footer { index_offset: pos, index_len: stored.len() as u64, body_len, index_hash: hash(&stored) };
    out.write_all(&footer.encode())?;
    out.flush()?;
    Ok(body_len)
}

pub fn collect(inputs: &[PathBuf], exclude: &[PathBuf]) -> Result<(Vec<Source>, Vec<String>)> {
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    let mut roots = std::collections::HashSet::new();
    let exclude: Vec<PathBuf> = exclude.iter().filter_map(|p| std::path::absolute(p).ok()).collect();
    for input in inputs {
        let full = std::path::absolute(input)?;
        let base = full
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("{} hat keinen Namen", input.display()))?;
        if !roots.insert(base.to_lowercase()) {
            bail!("zwei Eingaben heissen gleich: {base}");
        }
        let meta = std::fs::metadata(&full).with_context(|| format!("nicht gefunden: {}", input.display()))?;
        if meta.is_file() {
            files.push(Source { arc: base, path: full, size: meta.len() });
        } else {
            dirs.push(base.clone());
            walk(&full, &base, &exclude, &mut files, &mut dirs);
        }
    }
    files.sort_by_cached_key(|s| {
        let lower = s.arc.to_lowercase();
        let name = lower.rsplit('/').next().unwrap_or("").to_string();
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
        (ext, name, lower)
    });
    Ok((files, dirs))
}

fn walk(dir: &Path, prefix: &str, exclude: &[PathBuf], files: &mut Vec<Source>, dirs: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let arc = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() || exclude.contains(&path) {
            continue;
        }
        if kind.is_dir() {
            dirs.push(arc.clone());
            walk(&path, &arc, exclude, files, dirs);
        } else if let Ok(meta) = entry.metadata() {
            files.push(Source { arc, path, size: meta.len() });
        }
    }
}
