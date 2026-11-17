use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub struct Progress {
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub stage_total: AtomicU64,
    pub stage_done: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn set_total(&self, bytes: u64) {
        self.total.store(bytes, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.stage_total.store(0, Ordering::Relaxed);
        self.stage_done.store(0, Ordering::Relaxed);
    }

    pub fn add(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn fraction(&self) -> f32 {
        let ratio = |done: &AtomicU64, total: &AtomicU64| {
            let total = total.load(Ordering::Relaxed);
            if total == 0 { 0.0 } else { (done.load(Ordering::Relaxed) as f32 / total as f32).min(1.0) }
        };
        let read = ratio(&self.done, &self.total);
        if self.stage_total.load(Ordering::Relaxed) == 0 {
            return read;
        }
        0.55 * read + 0.45 * ratio(&self.stage_done, &self.stage_total) * read
    }

    pub fn check(&self) -> anyhow::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            anyhow::bail!("abgebrochen");
        }
        Ok(())
    }
}
