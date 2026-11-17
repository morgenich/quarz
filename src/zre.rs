use libz_sys as z;
use serde::{Deserialize, Serialize};
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::{c_int, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zlib {
    pub level: u8,
    pub mem: u8,
    pub strategy: u8,
}

const HEADER: usize = 16;

unsafe extern "C" fn zalloc(_opaque: *mut c_void, items: z::uInt, size: z::uInt) -> *mut c_void {
    let bytes = items as usize * size as usize + HEADER;
    let Ok(layout) = Layout::from_size_align(bytes, HEADER) else { return std::ptr::null_mut() };
    unsafe {
        let base = alloc(layout);
        if base.is_null() {
            return std::ptr::null_mut();
        }
        (base as *mut usize).write(bytes);
        base.add(HEADER) as *mut c_void
    }
}

unsafe extern "C" fn zfree(_opaque: *mut c_void, address: *mut c_void) {
    if address.is_null() {
        return;
    }
    unsafe {
        let base = (address as *mut u8).sub(HEADER);
        let bytes = (base as *const usize).read();
        dealloc(base, Layout::from_size_align_unchecked(bytes, HEADER));
    }
}

struct Encoder {
    stream: Box<z::z_stream>,
}

impl Encoder {
    fn new(settings: Zlib) -> Option<Encoder> {
        let mut stream = Box::new(z::z_stream {
            next_in: std::ptr::null_mut(),
            avail_in: 0,
            total_in: 0,
            next_out: std::ptr::null_mut(),
            avail_out: 0,
            total_out: 0,
            msg: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            zalloc,
            zfree,
            opaque: std::ptr::null_mut(),
            data_type: 0,
            adler: 0,
            reserved: 0,
        });
        let version = unsafe { z::zlibVersion() };
        let result = unsafe {
            z::deflateInit2_(
                stream.as_mut(),
                settings.level as c_int,
                z::Z_DEFLATED,
                -15,
                settings.mem as c_int,
                settings.strategy as c_int,
                version,
                std::mem::size_of::<z::z_stream>() as c_int,
            )
        };
        (result == z::Z_OK).then_some(Encoder { stream })
    }

    fn run(&mut self, input: &[u8], finish: bool, out: &mut Vec<u8>) -> bool {
        self.stream.next_in = input.as_ptr() as *mut u8;
        self.stream.avail_in = input.len() as z::uInt;
        loop {
            if out.capacity() - out.len() < 1 << 15 {
                out.reserve(1 << 16);
            }
            let room = (out.capacity() - out.len()).min(u32::MAX as usize);
            self.stream.next_out = unsafe { out.as_mut_ptr().add(out.len()) };
            self.stream.avail_out = room as z::uInt;
            let flush = if finish { z::Z_FINISH } else { z::Z_NO_FLUSH };
            let result = unsafe { z::deflate(self.stream.as_mut(), flush) };
            let written = room - self.stream.avail_out as usize;
            unsafe { out.set_len(out.len() + written) };
            match result {
                z::Z_STREAM_END => return true,
                z::Z_OK | z::Z_BUF_ERROR => {
                    if !finish && self.stream.avail_in == 0 && self.stream.avail_out != 0 {
                        return true;
                    }
                    if result == z::Z_BUF_ERROR && self.stream.avail_in == 0 && !finish {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { z::deflateEnd(self.stream.as_mut()) };
    }
}

pub fn deflate(plain: &[u8], settings: Zlib) -> Option<Vec<u8>> {
    let mut encoder = Encoder::new(settings)?;
    let mut out = Vec::with_capacity(plain.len() / 2 + 1024);
    encoder.run(plain, true, &mut out).then_some(out)
}

fn reproduces(plain: &[u8], comp: &[u8], settings: Zlib) -> bool {
    let Some(mut encoder) = Encoder::new(settings) else { return false };
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut checked = 0usize;
    let mut step = 1usize << 13;
    while pos < plain.len() {
        let end = (pos + step).min(plain.len());
        if !encoder.run(&plain[pos..end], false, &mut out) {
            return false;
        }
        if out.len() > comp.len() || out[checked..] != comp[checked..out.len()] {
            return false;
        }
        if out.len() > checked {
            step = (step * 2).min(1 << 22);
        }
        checked = out.len();
        pos = end;
    }
    encoder.run(&[], true, &mut out) && out == comp
}

fn candidates() -> Vec<Zlib> {
    let mut list = Vec::new();
    for strategy in [0u8, 1] {
        for mem in [8u8, 9] {
            for level in [6u8, 9, 1, 5, 7, 8, 4, 3, 2] {
                list.push(Zlib { level, mem, strategy });
            }
        }
    }
    list
}

static RECENT: Mutex<Vec<Zlib>> = Mutex::new(Vec::new());
static HITS: AtomicU32 = AtomicU32::new(0);
static MISSES: AtomicU32 = AtomicU32::new(0);

fn level_hint(settings: &Zlib) -> u8 {
    match (settings.strategy, settings.level) {
        (s, _) if s >= 2 => 0,
        (_, 0 | 1) => 0,
        (_, 2..=5) => 1,
        (_, 6) => 2,
        _ => 3,
    }
}

pub fn find(plain: &[u8], comp: &[u8], hint: u8) -> Option<Zlib> {
    let fits = |s: &Zlib| level_hint(s) == hint;
    let recent: Vec<Zlib> = RECENT.lock().map(|r| r.clone()).unwrap_or_default().into_iter().filter(fits).collect();
    let (hits, misses) = (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed));
    let search = misses <= 32 + 4 * hits;
    let all: Vec<Zlib> = if search { candidates().into_iter().filter(fits).collect() } else { Vec::new() };
    let found = try_settings(plain, comp, recent.into_iter().chain(all));
    if found.is_some() {
        HITS.fetch_add(1, Ordering::Relaxed);
    } else {
        MISSES.fetch_add(1, Ordering::Relaxed);
    }
    found
}

fn try_settings(plain: &[u8], comp: &[u8], list: impl Iterator<Item = Zlib>) -> Option<Zlib> {
    let mut tried = Vec::new();
    for settings in list {
        if tried.contains(&settings) {
            continue;
        }
        tried.push(settings);
        if reproduces(plain, comp, settings) {
            if let Ok(mut recent) = RECENT.lock() {
                recent.retain(|s| *s != settings);
                recent.insert(0, settings);
                recent.truncate(4);
            }
            return Some(settings);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_recreates_every_setting() {
        let text: Vec<u8> = (0..200_000u32).flat_map(|i| format!("Zeile {} mit Wert {}\n", i, i * 7 % 1000).into_bytes()).collect();
        for settings in candidates() {
            let comp = deflate(&text, settings).unwrap();
            let found = try_settings(&text, &comp, candidates().into_iter()).expect("settings found");
            assert_eq!(deflate(&text, found).unwrap(), comp);
        }
    }

    #[test]
    fn rejects_foreign_streams() {
        let text = b"abc".repeat(10_000);
        let mut comp = deflate(&text, Zlib { level: 6, mem: 8, strategy: 0 }).unwrap();
        let last = comp.len() - 1;
        comp[last] ^= 1;
        assert!(try_settings(&text, &comp, candidates().into_iter()).is_none());
    }
}
