use crate::codec;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Class {
    Text,
    Bin,
    X86,
    Delta(u8),
    Store,
    Skel,
    Corr,
}

fn is_text(sample: &[u8]) -> bool {
    let mut odd = 0usize;
    for &b in sample {
        if b == 0 {
            return false;
        }
        if b < 32 && b != b'\t' && b != b'\r' && b != b'\n' || b == 127 {
            odd += 1;
        }
    }
    odd * 20 < sample.len()
}

pub fn classify(data: &[u8], store_above: f64) -> Class {
    let n = data.len();
    let sample: Vec<u8> = if n > 32768 {
        let step = (n - 4096) / 7;
        (0..8).flat_map(|i| data[i * step..i * step + 4096].iter().copied()).collect()
    } else {
        data.to_vec()
    };
    let size = sample.len();
    if is_text(&sample) {
        return Class::Text;
    }
    if n < 4096 {
        return Class::Bin;
    }
    let quick = codec::quick_size(&sample);
    let packed = quick as f64 > 0.9 * size as f64
        && codec::careful_size(&sample) as f64 > store_above * size as f64;
    if data.starts_with(b"MZ") || data.starts_with(b"\x7fELF") {
        return if packed { Class::Store } else { Class::X86 };
    }
    if n >= 65536 && quick as f64 > 0.4 * size as f64 {
        let mut best = (quick, 0u8);
        for dist in 1..=4u8 {
            let cost = codec::quick_delta_size(&sample, dist as usize);
            if cost < best.0 {
                best = (cost, dist);
            }
        }
        if best.1 != 0 && (best.0 as f64) < 0.94 * quick as f64 {
            return Class::Delta(best.1);
        }
    }
    if packed { Class::Store } else { Class::Bin }
}
