use crate::progress::Progress;
use crate::unpack::ReadSeek;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::io::{Seek, SeekFrom, Write};

const START_MAGIC: &[u8; 4] = b"QZRS";
const END_MAGIC: &[u8; 4] = b"QZRE";
pub const TRAILER_LEN: u64 = 20;
const MAX_SHARD: u64 = 1 << 20;
const MIN_SHARD: u64 = 4096;
const STRIPE_SHARDS: u64 = 200;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Table {
    percent: u8,
    shard: u64,
    data_len: u64,
    stripes: u64,
    parity: u64,
    hashes: Vec<[u8; 8]>,
}

impl Table {
    fn data_shards(&self) -> u64 {
        self.data_len.div_ceil(self.shard)
    }

    fn members(&self, stripe: u64) -> Vec<u64> {
        (stripe..self.data_shards()).step_by(self.stripes as usize).collect()
    }

    fn parity_offset(&self, stripe: u64, nth: u64) -> u64 {
        (stripe * self.parity + nth) * self.shard
    }

    fn parity_hash(&self, stripe: u64, nth: u64) -> [u8; 8] {
        self.hashes[(self.data_shards() + stripe * self.parity + nth) as usize]
    }
}

struct Section {
    table: Table,
    data_start: u64,
    parity_start: u64,
}

pub struct Report {
    pub percent: u8,
    pub data_shards: u64,
    pub damaged: u64,
    pub repaired: u64,
    pub lost_stripes: u64,
}

fn short_hash(data: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&blake3::hash(data).as_bytes()[..8]);
    out
}

fn encode_table(table: &Table) -> Result<Vec<u8>> {
    let mut out = postcard::to_stdvec(table)?;
    let hash = short_hash(&out);
    out.extend_from_slice(&hash);
    Ok(out)
}

fn decode_table(raw: &[u8]) -> Option<Table> {
    if raw.len() < 9 {
        return None;
    }
    let (body, hash) = raw.split_at(raw.len() - 8);
    if short_hash(body) != hash {
        return None;
    }
    let table: Table = postcard::from_bytes(body).ok()?;
    let sane = table.shard >= 2 && table.shard % 2 == 0 && table.stripes > 0 && table.parity > 0;
    (sane && table.hashes.len() as u64 == table.data_shards() + table.stripes * table.parity).then_some(table)
}

fn read_exact_at(io: &mut dyn ReadSeek, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    io.seek(SeekFrom::Start(offset))?;
    io.read_exact(buf)
}

fn read_shard(io: &mut dyn ReadSeek, data_start: u64, table: &Table, index: u64) -> Result<Vec<u8>> {
    let mut shard = vec![0u8; table.shard as usize];
    let offset = index * table.shard;
    let len = (table.data_len - offset).min(table.shard) as usize;
    read_exact_at(io, data_start + offset, &mut shard[..len])?;
    Ok(shard)
}

pub fn section_len(io: &mut dyn ReadSeek, total: u64) -> Result<u64> {
    if total < TRAILER_LEN {
        return Ok(0);
    }
    let mut trailer = [0u8; TRAILER_LEN as usize];
    read_exact_at(io, total - TRAILER_LEN, &mut trailer)?;
    if &trailer[16..20] != END_MAGIC {
        return Ok(0);
    }
    let len = u64::from_le_bytes(trailer[8..16].try_into().unwrap());
    if len > total || len < TRAILER_LEN {
        bail!("Wiederherstellungsdaten sind beschaedigt");
    }
    Ok(len)
}

pub fn append(
    io: &mut dyn ReadSeek, data_start: u64, data_len: u64, percent: u8, out: &mut dyn Write, progress: &Progress,
) -> Result<u64> {
    let percent = percent.clamp(1, 100);
    let mut shard = data_len.div_ceil(STRIPE_SHARDS).clamp(MIN_SHARD, MAX_SHARD);
    shard = shard.div_ceil(64) * 64;
    let shards = data_len.div_ceil(shard).max(1);
    let stripes = shards.div_ceil(STRIPE_SHARDS);
    let per_stripe = shards.div_ceil(stripes);
    let parity = (per_stripe * percent as u64).div_ceil(100).max(1);
    let mut table = Table { percent, shard, data_len, stripes, parity, hashes: vec![[0u8; 8]; shards as usize] };
    progress.set_total(data_len);

    let mut all_parity: Vec<u8> = Vec::with_capacity((stripes * parity * shard) as usize);
    let mut parity_hashes = Vec::new();
    for stripe in 0..stripes {
        progress.check()?;
        let members = table.members(stripe);
        let mut data = Vec::with_capacity(members.len());
        for &index in &members {
            let block = read_shard(io, data_start, &table, index)?;
            table.hashes[index as usize] = short_hash(&block);
            progress.add(shard);
            data.push(block);
        }
        let recovery = reed_solomon_simd::encode(data.len(), parity as usize, &data)
            .map_err(|e| anyhow!("Wiederherstellungsdaten: {e}"))?;
        for block in recovery {
            parity_hashes.push(short_hash(&block));
            all_parity.extend_from_slice(&block);
        }
    }
    table.hashes.extend(parity_hashes);

    let encoded = encode_table(&table)?;
    let section = 4 + 8 + encoded.len() as u64 + all_parity.len() as u64 + encoded.len() as u64 + TRAILER_LEN;
    out.write_all(START_MAGIC)?;
    out.write_all(&(encoded.len() as u64).to_le_bytes())?;
    out.write_all(&encoded)?;
    out.write_all(&all_parity)?;
    out.write_all(&encoded)?;
    out.write_all(&(encoded.len() as u64).to_le_bytes())?;
    out.write_all(&section.to_le_bytes())?;
    out.write_all(END_MAGIC)?;
    out.flush()?;
    Ok(section)
}

fn locate(io: &mut dyn ReadSeek, total: u64) -> Result<Option<Section>> {
    let section_at = |io: &mut dyn ReadSeek, start: u64, table: Table, table_len: u64| -> Option<Section> {
        let parity_start = start + 12 + table_len;
        let data_start = start.checked_sub(table.data_len)?;
        let _ = io;
        Some(Section { table, data_start, parity_start })
    };
    if total >= TRAILER_LEN {
        let mut trailer = [0u8; TRAILER_LEN as usize];
        read_exact_at(io, total - TRAILER_LEN, &mut trailer)?;
        let table_len = u64::from_le_bytes(trailer[0..8].try_into().unwrap());
        let len = u64::from_le_bytes(trailer[8..16].try_into().unwrap());
        if &trailer[16..20] == END_MAGIC && len <= total && table_len < len {
            let start = total - len;
            let mut raw = vec![0u8; table_len as usize];
            for offset in [total - TRAILER_LEN - table_len, start + 12] {
                if read_exact_at(io, offset, &mut raw).is_ok() {
                    if let Some(table) = decode_table(&raw) {
                        return Ok(section_at(io, start, table, table_len));
                    }
                }
            }
        }
    }
    let mut window = vec![0u8; 1 << 20];
    let mut pos = total;
    while pos > 0 {
        let from = pos.saturating_sub(window.len() as u64 - 16);
        let len = (pos - from) as usize;
        read_exact_at(io, from, &mut window[..len])?;
        let mut end = len;
        while let Some(at) = memchr::memmem::rfind(&window[..end], START_MAGIC) {
            end = at;
            let start = from + at as u64;
            let mut head = [0u8; 8];
            if start + 12 > total || read_exact_at(io, start + 4, &mut head).is_err() {
                continue;
            }
            let table_len = u64::from_le_bytes(head);
            if table_len > 64 << 20 || start + 12 + table_len > total {
                continue;
            }
            let mut raw = vec![0u8; table_len as usize];
            if read_exact_at(io, start + 12, &mut raw).is_ok() {
                if let Some(table) = decode_table(&raw) {
                    return Ok(section_at(io, start, table, table_len));
                }
            }
        }
        pos = from;
    }
    Ok(None)
}

fn scan(io: &mut dyn ReadSeek, total: u64, mut out: Option<&mut std::fs::File>, progress: &Progress) -> Result<Option<Report>> {
    let Some(section) = locate(io, total)? else { return Ok(None) };
    let table = &section.table;
    let mut report =
        Report { percent: table.percent, data_shards: table.data_shards(), damaged: 0, repaired: 0, lost_stripes: 0 };
    progress.set_total(table.data_len);
    for stripe in 0..table.stripes {
        progress.check()?;
        let members = table.members(stripe);
        let mut good = Vec::with_capacity(members.len());
        let mut missing = Vec::new();
        for (slot, &index) in members.iter().enumerate() {
            let offset = index * table.shard;
            let readable = section.data_start + offset < total;
            let block = if readable { read_shard(io, section.data_start, table, index).ok() } else { None };
            match block {
                Some(block) if short_hash(&block) == table.hashes[index as usize] => good.push((slot, block)),
                _ => missing.push((slot, index)),
            }
            progress.add(table.shard);
        }
        if missing.is_empty() {
            continue;
        }
        report.damaged += missing.len() as u64;
        let mut recovery = Vec::new();
        for nth in 0..table.parity {
            let mut block = vec![0u8; table.shard as usize];
            let offset = section.parity_start + table.parity_offset(stripe, nth);
            if read_exact_at(io, offset, &mut block).is_ok() && short_hash(&block) == table.parity_hash(stripe, nth) {
                recovery.push((nth as usize, block));
            }
        }
        if recovery.len() < missing.len() {
            report.lost_stripes += 1;
            continue;
        }
        let Some(out) = out.as_deref_mut() else {
            report.repaired += missing.len() as u64;
            continue;
        };
        let restored = reed_solomon_simd::decode(members.len(), table.parity as usize, good, recovery)
            .map_err(|e| anyhow!("Wiederherstellung: {e}"))?;
        for (slot, index) in missing {
            let block = restored.get(&slot).ok_or_else(|| anyhow!("Teil {index} fehlt nach der Wiederherstellung"))?;
            if short_hash(block) != table.hashes[index as usize] {
                bail!("Teil {index} ist nach der Wiederherstellung immer noch falsch");
            }
            let offset = index * table.shard;
            let len = (table.data_len - offset).min(table.shard) as usize;
            out.seek(SeekFrom::Start(section.data_start + offset))?;
            out.write_all(&block[..len])?;
            report.repaired += 1;
        }
    }
    Ok(Some(report))
}

pub fn check(io: &mut dyn ReadSeek, total: u64, progress: &Progress) -> Result<Option<Report>> {
    scan(io, total, None, progress)
}

pub fn repair(io: &mut dyn ReadSeek, total: u64, out: &mut std::fs::File, progress: &Progress) -> Result<Report> {
    scan(io, total, Some(out), progress)
        .context("Reparatur")?
        .ok_or_else(|| anyhow!("das Archiv enthaelt keine Wiederherstellungsdaten"))
}

pub fn percent(io: &mut dyn ReadSeek, total: u64) -> Option<u8> {
    locate(io, total).ok().flatten().map(|s| s.table.percent)
}
