use crate::classify::Class;
use crate::format::Codec;
use anyhow::{Result, anyhow, bail};
use liblzma::stream::{Action, Check, Filters, LzmaOptions, Status, Stream};
use zstd::zstd_safe::{CParameter, DParameter};

const LZMA_EXTREME: u32 = 1 << 31;

#[derive(Clone, Copy, Debug)]
pub enum Backend {
    Store,
    Zstd(i32),
    Lzma(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct Level {
    pub id: u8,
    pub name: &'static str,
    pub backend: Backend,
    pub dict: u32,
    pub block: usize,
    pub store_above: f64,
    pub unwrap: bool,
}

pub const LEVELS: [Level; 6] = [
    Level { id: 0, name: "Speichern", backend: Backend::Store, dict: 0, block: 64 << 20, store_above: 0.0, unwrap: false },
    Level { id: 1, name: "Am schnellsten", backend: Backend::Zstd(3), dict: 8 << 20, block: 16 << 20, store_above: 0.90, unwrap: false },
    Level { id: 2, name: "Schnell", backend: Backend::Zstd(9), dict: 32 << 20, block: 32 << 20, store_above: 0.95, unwrap: true },
    Level { id: 3, name: "Normal", backend: Backend::Lzma(1), dict: 32 << 20, block: 32 << 20, store_above: 0.99, unwrap: true },
    Level { id: 4, name: "Gut", backend: Backend::Lzma(6), dict: 32 << 20, block: 32 << 20, store_above: 0.995, unwrap: true },
    Level { id: 5, name: "Beste", backend: Backend::Lzma(9 | LZMA_EXTREME), dict: 128 << 20, block: 128 << 20, store_above: 0.999, unwrap: true },
];

pub fn level(id: u8) -> Level {
    let mut level = LEVELS[(id as usize).min(LEVELS.len() - 1)];
    if let Some(spec) = std::env::var("QZ_BACKEND").ok().as_deref().and_then(|s| s.split_once(':')) {
        match (spec.0, spec.1.parse::<i32>()) {
            ("zstd", Ok(n)) => level.backend = Backend::Zstd(n),
            ("lzma", Ok(n)) => level.backend = Backend::Lzma(n as u32),
            _ => {}
        }
    }
    level
}

impl Level {
    pub fn worker_memory(&self) -> u64 {
        match self.backend {
            Backend::Lzma(_) => self.dict as u64 * 12 + 2 * self.block as u64,
            _ => self.dict as u64 * 4 + 2 * self.block as u64,
        }
    }
}

fn delta_encode(data: &[u8], dist: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..dist.min(data.len())]);
    for i in dist..data.len() {
        out.push(data[i].wrapping_sub(data[i - dist]));
    }
    out
}

fn delta_decode(data: &mut [u8], dist: usize) {
    for i in dist..data.len() {
        data[i] = data[i].wrapping_add(data[i - dist]);
    }
}

fn run_lzma(stream: &mut Stream, data: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(capacity.max(1 << 12));
    let mut pos = 0usize;
    loop {
        if out.capacity() - out.len() < 1 << 15 {
            out.reserve((out.len() / 2).max(1 << 20));
        }
        let before = stream.total_in();
        let status = stream
            .process_vec(&data[pos..], &mut out, Action::Finish)
            .map_err(|e| anyhow!("LZMA: {e}"))?;
        pos += (stream.total_in() - before) as usize;
        if matches!(status, Status::StreamEnd) {
            return Ok(out);
        }
    }
}

fn lzma_encode(data: &[u8], preset: u32, dict: u32, class: Class) -> Result<Vec<u8>> {
    let mut opts = LzmaOptions::new_preset(preset).map_err(|e| anyhow!("LZMA: {e}"))?;
    let mut size = 1u32 << 16;
    while (size as usize) < data.len() && size < dict {
        size <<= 1;
    }
    opts.dict_size(size);
    if class == Class::Text {
        opts.position_bits(0);
    }
    let mut filters = Filters::new();
    match class {
        Class::Delta(dist) => {
            filters.delta_properties(&[dist - 1]).map_err(|e| anyhow!("LZMA: {e}"))?;
        }
        Class::X86 => {
            filters.x86();
        }
        _ => {}
    }
    filters.lzma2(&opts);
    let mut stream = Stream::new_stream_encoder(&filters, Check::None).map_err(|e| anyhow!("LZMA: {e}"))?;
    run_lzma(&mut stream, data, data.len() / 3)
}

fn zstd_encode(data: &[u8], level: i32, dict: u32) -> Result<Vec<u8>> {
    let mut window = 20u32;
    while (1u64 << window) < data.len() as u64 && (1u64 << window) < dict as u64 {
        window += 1;
    }
    let mut c = zstd::bulk::Compressor::new(level)?;
    c.set_parameter(CParameter::WindowLog(window))?;
    c.set_parameter(CParameter::EnableLongDistanceMatching(window >= 24))?;
    Ok(c.compress(data)?)
}

pub fn compress(class: Class, data: &[u8], level: &Level) -> Result<(Codec, Vec<u8>)> {
    let packed = match (level.backend, class) {
        (Backend::Store, _) | (_, Class::Store) => None,
        (Backend::Zstd(l), Class::Delta(dist)) => {
            Some((Codec::ZstdDelta(dist), zstd_encode(&delta_encode(data, dist as usize), l, level.dict)?))
        }
        (Backend::Zstd(l), _) => Some((Codec::Zstd, zstd_encode(data, l, level.dict)?)),
        (Backend::Lzma(p), _) => {
            let codec = match class {
                Class::Delta(dist) => Codec::LzmaDelta(dist),
                Class::X86 => Codec::LzmaX86,
                _ => Codec::Lzma,
            };
            Some((codec, lzma_encode(data, p, level.dict, class)?))
        }
    };
    match packed {
        Some((codec, out)) if out.len() < data.len() => Ok((codec, out)),
        _ => Ok((Codec::Store, data.to_vec())),
    }
}

pub fn decompress(codec: Codec, data: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    let out = match codec {
        Codec::Store => data.to_vec(),
        Codec::Zstd | Codec::ZstdDelta(_) => {
            let mut d = zstd::bulk::Decompressor::new()?;
            d.set_parameter(DParameter::WindowLogMax(31))?;
            let mut out = d.decompress(data, raw_len)?;
            if let Codec::ZstdDelta(dist) = codec {
                delta_decode(&mut out, dist as usize);
            }
            out
        }
        Codec::Lzma | Codec::LzmaX86 | Codec::LzmaDelta(_) => {
            let mut stream = Stream::new_stream_decoder(u64::MAX, 0).map_err(|e| anyhow!("LZMA: {e}"))?;
            run_lzma(&mut stream, data, raw_len + 64)?
        }
    };
    if out.len() != raw_len {
        bail!("Block hat die falsche Laenge ({} statt {})", out.len(), raw_len);
    }
    Ok(out)
}

pub fn estimate(sample: &[u8], level: &Level) -> usize {
    let packed = match level.backend {
        Backend::Store => return sample.len(),
        Backend::Zstd(l) => zstd_encode(sample, l, level.dict).map(|v| v.len()),
        Backend::Lzma(p) => lzma_encode(sample, p, level.dict, Class::Bin).map(|v| v.len().saturating_sub(32)),
    };
    packed.unwrap_or(sample.len())
}

pub fn quick_size(data: &[u8]) -> usize {
    zstd::bulk::compress(data, 1).map(|v| v.len()).unwrap_or(data.len())
}

pub fn careful_size(data: &[u8]) -> usize {
    let attempt = || -> Result<usize> {
        let mut opts = LzmaOptions::new_preset(1).map_err(|e| anyhow!("{e}"))?;
        opts.dict_size(1 << 16);
        let mut filters = Filters::new();
        filters.lzma2(&opts);
        let mut stream = Stream::new_stream_encoder(&filters, Check::None).map_err(|e| anyhow!("{e}"))?;
        Ok(run_lzma(&mut stream, data, data.len() + 128)?.len().saturating_sub(32))
    };
    attempt().unwrap_or(data.len())
}

pub fn quick_delta_size(data: &[u8], dist: usize) -> usize {
    quick_size(&delta_encode(data, dist))
}
