use crate::classify::{Class, classify};
use crate::codec::{self, Level};
use crate::format::Kind;
use crate::{Hash, hash};
use aho_corasick::{AhoCorasick, Input, MatchKind};
use rayon::prelude::*;
use std::collections::HashMap;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

pub const MAX_DEPTH: u32 = 4;
const MIN_BLOB: usize = 64;
const MIN_STREAM: usize = 32;
const MIN_JPEG: usize = 2048;
const PLAIN_LIMIT: usize = 256 << 20;
const BIG_STREAM: usize = 256 << 10;
const SAMPLE: usize = 256 << 10;
const QUICK_LIMIT: usize = 64 << 10;

static TRACE: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

pub fn trace() -> String {
    let t: Vec<u64> = TRACE.iter().map(|a| a.load(Ordering::Relaxed)).collect();
    format!("zlib: {} Treffer, {} Fehlversuche, {:.1} s | preflate: {} Stroeme, {:.1} s", t[0], t[1], t[2] as f64 / 1e6, t[3], t[4] as f64 / 1e6)
}
const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

pub enum Tree {
    Leaf { hash: Hash, data: Vec<u8>, class: Option<Class> },
    Comp { hash: Hash, data: Vec<u8>, pieces: Vec<Piece> },
}

pub enum Piece {
    Raw(Range<usize>),
    Stream {
        range: Range<usize>,
        kind: Kind,
        child: Box<Tree>,
        corrections: Vec<u8>,
        doubtful: bool,
    },
}

impl Tree {
    pub fn hash(&self) -> Hash {
        match self {
            Tree::Leaf { hash, .. } | Tree::Comp { hash, .. } => *hash,
        }
    }

    pub fn size(&self) -> usize {
        match self {
            Tree::Leaf { data, .. } | Tree::Comp { data, .. } => data.len(),
        }
    }
}

pub struct Ctx<'a> {
    pub known: &'a HashMap<Hash, u32>,
    pub level: &'a Level,
}

#[derive(Clone, Copy)]
enum Pat {
    Zip,
    Gzip,
    Png,
    Jpeg,
    Zlib,
}

fn searcher() -> &'static (AhoCorasick, Vec<Pat>) {
    static SEARCHER: OnceLock<(AhoCorasick, Vec<Pat>)> = OnceLock::new();
    SEARCHER.get_or_init(|| {
        let mut patterns: Vec<Vec<u8>> =
            vec![b"PK\x03\x04".to_vec(), b"\x1f\x8b\x08".to_vec(), PNG_SIG.to_vec(), b"\xff\xd8\xff".to_vec()];
        let mut kinds = vec![Pat::Zip, Pat::Gzip, Pat::Png, Pat::Jpeg];
        for cinfo in 1u32..=7 {
            let cmf = (cinfo << 4) | 8;
            for flg in 0u32..256 {
                if flg & 0x20 == 0 && (cmf * 256 + flg) % 31 == 0 {
                    patterns.push(vec![cmf as u8, flg as u8]);
                    kinds.push(Pat::Zlib);
                }
            }
        }
        let ac = AhoCorasick::builder().match_kind(MatchKind::LeftmostFirst).build(&patterns).unwrap();
        (ac, kinds)
    })
}

struct Found {
    range: Range<usize>,
    what: What,
}

enum What {
    Deflate { quick: Option<u8> },
    Png { payload: Vec<u8>, sizes: Vec<(u32, u32)> },
    Jpeg,
}

type Located = Option<(usize, Option<Found>)>;

fn u16le(data: &[u8], at: usize) -> usize {
    u16::from_le_bytes([data[at], data[at + 1]]) as usize
}

fn inflate_len(buf: &[u8], zlib: bool, waste: &mut u64) -> Option<(usize, u64)> {
    let mut d = flate2::Decompress::new(zlib);
    let mut scratch = vec![0u8; 1 << 16];
    loop {
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let status = d.decompress(&buf[before_in as usize..], &mut scratch, flate2::FlushDecompress::None);
        match status {
            Ok(flate2::Status::StreamEnd) => return Some((d.total_in() as usize, d.total_out())),
            Ok(_) if (d.total_in() != before_in || d.total_out() != before_out)
                && d.total_out() <= PLAIN_LIMIT as u64 => {}
            _ => {
                *waste += d.total_out();
                return None;
            }
        }
    }
}

fn deflate_found(start: usize, end: usize, produced: u64, quick: Option<u8>) -> (usize, Option<Found>) {
    let keep = end - start >= MIN_STREAM && produced >= MIN_BLOB as u64;
    (end, keep.then(|| Found { range: start..end, what: What::Deflate { quick } }))
}

fn zip_entry(data: &[u8], at: usize, waste: &mut u64) -> Located {
    if at + 30 > data.len() {
        return None;
    }
    let (flags, method) = (u16le(data, at + 6), u16le(data, at + 8));
    if method != 8 || flags & 1 != 0 {
        return None;
    }
    let start = at + 30 + u16le(data, at + 26) + u16le(data, at + 28);
    if start >= data.len() {
        return None;
    }
    let (len, produced) = inflate_len(&data[start..], false, waste)?;
    Some(deflate_found(start, start + len, produced, None))
}

fn gzip_member(data: &[u8], at: usize, waste: &mut u64) -> Located {
    let n = data.len();
    if at + 18 > n {
        return None;
    }
    let flags = data[at + 3];
    if flags & 0xE0 != 0 {
        return None;
    }
    let mut pos = at + 10;
    if flags & 4 != 0 {
        if pos + 2 > n {
            return None;
        }
        pos += 2 + u16le(data, pos);
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            let end = (pos + 65536).min(n);
            pos += memchr::memchr(0, data.get(pos..end)?)? + 1;
        }
    }
    if flags & 2 != 0 {
        pos += 2;
    }
    if pos >= n {
        return None;
    }
    let (len, produced) = inflate_len(&data[pos..], false, waste)?;
    Some(deflate_found(pos, pos + len, produced, None))
}

fn zlib_stream(data: &[u8], at: usize, waste: &mut u64) -> Located {
    let (len, produced) = inflate_len(&data[at..], true, waste)?;
    if len < 6 {
        return None;
    }
    let image = at >= 4 && &data[at - 4..at] == b"IDAT";
    let hint = (!image).then_some(data[at + 1] >> 6);
    let (_, found) = deflate_found(at + 2, at + len - 4, produced, hint);
    Some((at + len, found))
}

fn png(data: &[u8], at: usize, waste: &mut u64) -> Located {
    let n = data.len();
    let mut pos = at + 8;
    let mut idat: Vec<(usize, usize)> = Vec::new();
    while pos + 12 <= n {
        let length = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let end = pos + 12 + length;
        if end > n || !ctype.iter().all(u8::is_ascii_alphabetic) {
            break;
        }
        if ctype == b"IDAT" {
            idat.push((pos, length));
        } else if !idat.is_empty() {
            break;
        }
        pos = end;
    }
    if idat.len() < 2 {
        return None;
    }
    let start = idat[0].0;
    let stop = idat[idat.len() - 1].0 + 12 + idat[idat.len() - 1].1;
    let skip = Some((stop, None));
    let mut payload = Vec::new();
    for &(chunk, length) in &idat {
        let stored = u32::from_be_bytes(data[chunk + 8 + length..chunk + 12 + length].try_into().unwrap());
        if crc32fast::hash(&data[chunk + 4..chunk + 8 + length]) != stored {
            return skip;
        }
        payload.extend_from_slice(&data[chunk + 8..chunk + 8 + length]);
    }
    if payload.len() < MIN_STREAM + 6 || payload[0] & 15 != 8 {
        return skip;
    }
    match inflate_len(&payload, true, waste) {
        Some((len, produced)) if len == payload.len() && produced >= MIN_BLOB as u64 => {}
        _ => return skip,
    }
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for &(_, length) in &idat {
        match sizes.last_mut() {
            Some(last) if last.0 == length as u32 => last.1 += 1,
            _ => sizes.push((length as u32, 1)),
        }
    }
    Some((stop, Some(Found { range: start..stop, what: What::Png { payload, sizes } })))
}

fn jpeg_extent(buf: &[u8]) -> Option<usize> {
    let n = buf.len();
    let mut pos = 2;
    let mut scans = 0;
    loop {
        if pos + 4 > n || buf[pos] != 0xFF {
            return None;
        }
        let marker = buf[pos + 1];
        match marker {
            0xD9 => return (scans > 0).then_some(pos + 2),
            0xFF => {
                pos += 1;
                continue;
            }
            0x01 | 0xD0..=0xD7 => {
                pos += 2;
                continue;
            }
            0x00 | 0xD8 => return None,
            _ => {}
        }
        let length = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        if length < 2 {
            return None;
        }
        pos += 2 + length;
        if pos > n {
            return None;
        }
        if marker == 0xDA {
            scans += 1;
            loop {
                pos += memchr::memchr(0xFF, &buf[pos..])?;
                let next = *buf.get(pos + 1)?;
                if next == 0 || (0xD0..=0xD7).contains(&next) {
                    pos += 2;
                } else if next == 0xFF {
                    pos += 1;
                } else {
                    break;
                }
            }
        }
    }
}

fn jpeg(data: &[u8], at: usize) -> Located {
    let len = jpeg_extent(&data[at..])?;
    Some((at + len, (len >= MIN_JPEG).then(|| Found { range: at..at + len, what: What::Jpeg })))
}

fn locate(data: &[u8]) -> Vec<Found> {
    let (ac, kinds) = searcher();
    let mut found = Vec::new();
    let n = data.len();
    let mut pos = 0usize;
    let mut waste = 0u64;
    let waste_limit = 4 * n as u64 + (64 << 20);
    while pos < n && waste <= waste_limit {
        let Some(m) = ac.find(Input::new(data).span(pos..n)) else { break };
        let at = m.start();
        let located = match kinds[m.pattern().as_usize()] {
            Pat::Zip => zip_entry(data, at, &mut waste),
            Pat::Gzip => gzip_member(data, at, &mut waste),
            Pat::Png => png(data, at, &mut waste),
            Pat::Jpeg => jpeg(data, at),
            Pat::Zlib => zlib_stream(data, at, &mut waste),
        };
        match located {
            Some((next, hit)) => {
                found.extend(hit);
                pos = next.max(at + 1);
            }
            None => pos = at + 1,
        }
    }
    found
}

fn preflate(comp: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let config = preflate_rs::PreflateConfig {
        max_chain_length: 4096,
        plain_text_limit: PLAIN_LIMIT,
        verify_compression: true,
    };
    let mut processor = preflate_rs::PreflateStreamProcessor::new(&config);
    let result = processor.decompress(comp).ok()?;
    if !processor.is_done() || result.compressed_size != comp.len() {
        return None;
    }
    Some((processor.plain_text().text().to_vec(), result.corrections))
}

fn lepton(jpeg: &[u8]) -> Option<Vec<u8>> {
    let features = lepton_jpeg::EnabledFeatures::compat_lepton_vector_write();
    let pool = lepton_jpeg::SingleThreadPool::default();
    lepton_jpeg::encode_lepton_verify(jpeg, &features, &pool).ok().map(|(out, _)| out)
}

fn worth_it(comp_len: usize, plain: &[u8], corrections: usize, level: &Level, standalone: bool) -> bool {
    if !standalone || plain.len() < BIG_STREAM {
        if corrections * 8 <= comp_len {
            return true;
        }
        return corrections as f64 + 0.8 * codec::quick_size(plain) as f64 <= 0.95 * comp_len as f64;
    }
    let start = (plain.len() - SAMPLE) / 2;
    let packed = codec::estimate(&plain[start..start + SAMPLE], level) as f64;
    packed / SAMPLE as f64 * plain.len() as f64 + corrections as f64 <= 0.97 * comp_len as f64
}

fn inflate_raw(comp: &[u8]) -> Option<Vec<u8>> {
    let mut d = flate2::Decompress::new(false);
    let mut out = Vec::with_capacity(comp.len() * 4);
    loop {
        if out.capacity() - out.len() < 1 << 16 {
            out.reserve(out.len().max(1 << 16));
        }
        let before = (d.total_in(), d.total_out());
        let status = d.decompress_vec(&comp[d.total_in() as usize..], &mut out, flate2::FlushDecompress::Finish);
        match status {
            Ok(flate2::Status::StreamEnd) => return (d.total_in() as usize == comp.len()).then_some(out),
            Ok(_) if (d.total_in(), d.total_out()) != before && out.len() <= PLAIN_LIMIT => {}
            _ => return None,
        }
    }
}

fn deflate_piece(
    range: Range<usize>, comp: &[u8], kind: Kind, quick: Option<u8>, depth: u32, ctx: &Ctx,
) -> Option<Piece> {
    let clock = std::time::Instant::now();
    let quick = if let (Some(hint), true) = (quick, comp.len() <= QUICK_LIMIT) {
        inflate_raw(comp).and_then(|plain| crate::zre::find(&plain, comp, hint).map(|settings| (plain, settings)))
    } else {
        None
    };
    TRACE[if quick.is_some() { 0 } else { 1 }].fetch_add(1, Ordering::Relaxed);
    TRACE[2].fetch_add(clock.elapsed().as_micros() as u64, Ordering::Relaxed);
    let clock = std::time::Instant::now();
    let preflated = if quick.is_none() { preflate(comp) } else { None };
    if quick.is_none() {
        TRACE[3].fetch_add(1, Ordering::Relaxed);
        TRACE[4].fetch_add(clock.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
    let (plain, corrections, kind) = match quick {
        Some((plain, settings)) => {
            let kind = match kind {
                Kind::Png { prefix, suffix, sizes } => Kind::PngZlib { prefix, suffix, sizes, zlib: settings },
                _ => Kind::DeflateZlib(settings),
            };
            (plain, Vec::new(), kind)
        }
        None => {
            let (plain, corrections) = preflated?;
            (plain, corrections, kind)
        }
    };
    let standalone = matches!(kind, Kind::Png { .. } | Kind::PngZlib { .. });
    if !worth_it(comp.len(), &plain, corrections.len(), ctx.level, standalone) {
        return None;
    }
    let child = unwrap(plain, depth + 1, ctx);
    let doubtful = matches!(&child, Tree::Leaf { class: Some(Class::Store), data, .. } if data.len() >= comp.len());
    Some(Piece::Stream { range, kind, child: Box::new(child), corrections, doubtful })
}

fn process(data: &[u8], found: &Found, depth: u32, ctx: &Ctx) -> Option<Piece> {
    let range = found.range.clone();
    match &found.what {
        What::Deflate { quick } => deflate_piece(range.clone(), &data[range], Kind::Deflate, *quick, depth, ctx),
        What::Png { payload, sizes } => {
            let end = payload.len() - 4;
            let kind = Kind::Png {
                prefix: [payload[0], payload[1]],
                suffix: payload[end..].try_into().unwrap(),
                sizes: sizes.clone(),
            };
            deflate_piece(range, &payload[2..end], kind, None, depth, ctx)
        }
        What::Jpeg => {
            let jpeg = &data[range.clone()];
            let coded = lepton(jpeg)?;
            if coded.len() as f64 > 0.97 * jpeg.len() as f64 {
                return None;
            }
            let child = Tree::Leaf { hash: hash(&coded), data: coded, class: Some(Class::Store) };
            Some(Piece::Stream { range, kind: Kind::Jpeg, child: Box::new(child), corrections: Vec::new(), doubtful: false })
        }
    }
}

pub fn unwrap(data: Vec<u8>, depth: u32, ctx: &Ctx) -> Tree {
    let h = hash(&data);
    let known = ctx.known.contains_key(&h);
    if !known && ctx.level.unwrap && depth < MAX_DEPTH && data.len() >= MIN_BLOB {
        let found = locate(&data);
        if !found.is_empty() {
            let streams: Vec<Option<Piece>> = found
                .par_iter()
                .map(|f| catch_unwind(AssertUnwindSafe(|| process(&data, f, depth, ctx))).unwrap_or(None))
                .collect();
            let mut pieces = Vec::new();
            let mut pos = 0usize;
            for piece in streams.into_iter().flatten() {
                let Piece::Stream { range, .. } = &piece else { continue };
                if range.start > pos {
                    pieces.push(Piece::Raw(pos..range.start));
                }
                pos = range.end;
                pieces.push(piece);
            }
            if !pieces.is_empty() {
                if pos < data.len() {
                    pieces.push(Piece::Raw(pos..data.len()));
                }
                return Tree::Comp { hash: h, data, pieces };
            }
        }
    }
    let class = if known || data.is_empty() { None } else { Some(classify(&data, ctx.level.store_above)) };
    Tree::Leaf { hash: h, data, class }
}
