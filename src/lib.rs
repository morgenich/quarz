pub mod classify;
pub mod codec;
pub mod crypto;
pub mod foreign;
pub mod format;
pub mod gui;
pub mod lang;
pub mod ops;
pub mod pack;
pub mod progress;
pub mod recovery;
pub mod scan;
pub mod unpack;
pub mod volume;

#[derive(Debug, Clone, Copy)]
pub struct PasswordNeeded;

impl std::fmt::Display for PasswordNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passwort noetig")
    }
}

impl std::error::Error for PasswordNeeded {}

pub type Hash = [u8; 16];

pub fn hash(data: &[u8]) -> Hash {
    let mut out = [0u8; 16];
    out.copy_from_slice(&blake3::hash(data).as_bytes()[..16]);
    out
}

pub fn total_ram() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            return status.ullTotalPhys;
        }
    }
    8 << 30
}
