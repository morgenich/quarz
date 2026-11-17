use crate::Hash;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8; 4] = b"QZ2\0";
pub const END_MAGIC: &[u8; 4] = b"QZ2E";
pub const FOOTER_LEN: usize = 8 + 8 + 8 + 16 + 4;
pub const NO_NODE: u32 = u32::MAX;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Header {
    pub version: u32,
    pub crypt: Option<CryptHeader>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CryptHeader {
    pub salt: [u8; 16],
    pub mem_kib: u32,
    pub passes: u32,
    pub lanes: u32,
    pub check: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Store,
    Zstd,
    ZstdDelta(u8),
    Lzma,
    LzmaX86,
    LzmaDelta(u8),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BlockInfo {
    pub offset: u64,
    pub stored: u64,
    pub raw: u64,
    pub codec: Codec,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Extent {
    pub block: u32,
    pub offset: u64,
    pub len: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Kind {
    Deflate,
    Png { prefix: [u8; 2], suffix: [u8; 4], sizes: Vec<(u32, u32)> },
    Jpeg,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Insert {
    pub gap: u64,
    pub kind: Kind,
    pub child: u32,
    pub aux: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Node {
    Leaf(Vec<Extent>),
    Composite { skeleton: u32, size: u64, inserts: Vec<Insert> },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub attrs: u32,
    pub hash: Hash,
    pub parts: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Index {
    pub comment: String,
    pub created: i64,
    pub locked: bool,
    pub level: u8,
    pub blocks: Vec<BlockInfo>,
    pub nodes: Vec<Node>,
    pub files: Vec<FileEntry>,
    pub dirs: Vec<String>,
}

pub struct Footer {
    pub index_offset: u64,
    pub index_len: u64,
    pub body_len: u64,
    pub index_hash: Hash,
}

impl Footer {
    pub fn encode(&self) -> [u8; FOOTER_LEN] {
        let mut out = [0u8; FOOTER_LEN];
        out[0..8].copy_from_slice(&self.index_offset.to_le_bytes());
        out[8..16].copy_from_slice(&self.index_len.to_le_bytes());
        out[16..24].copy_from_slice(&self.body_len.to_le_bytes());
        out[24..40].copy_from_slice(&self.index_hash);
        out[40..44].copy_from_slice(END_MAGIC);
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Footer> {
        if raw.len() != FOOTER_LEN || &raw[40..44] != END_MAGIC {
            bail!("kein Quarz-Archiv (Endmarke fehlt)");
        }
        let mut index_hash = [0u8; 16];
        index_hash.copy_from_slice(&raw[24..40]);
        Ok(Footer {
            index_offset: u64::from_le_bytes(raw[0..8].try_into().unwrap()),
            index_len: u64::from_le_bytes(raw[8..16].try_into().unwrap()),
            body_len: u64::from_le_bytes(raw[16..24].try_into().unwrap()),
            index_hash,
        })
    }
}

pub fn encode_header(header: &Header) -> Result<Vec<u8>> {
    let body = postcard::to_stdvec(header)?;
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}
