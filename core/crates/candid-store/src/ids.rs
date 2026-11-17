use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

const HEX: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(data.as_ref()))
}

/// Lowercase hex SHA-256 of a file, read in 1 MiB chunks.
pub fn sha256_hex_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("system random source");
    b
}

fn format_uuid(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

static V7_STATE: Mutex<(u64, u16)> = Mutex::new((0, 0));

/// Lowercase UUIDv7 text, monotonic within a process (same layout as the Swift `UUIDv7.make`).
pub fn uuid_v7(ms: Option<i64>) -> String {
    let mut state = V7_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let now = ms.unwrap_or_else(crate::clock::system_now_ms);
    let mut stamp = now.max(0) as u64;
    if stamp <= state.0 {
        stamp = state.0;
        state.1 = state.1.wrapping_add(1);
    } else {
        state.0 = stamp;
        let r = random_bytes::<2>();
        state.1 = u16::from_be_bytes(r) % 0x0800;
    }
    let mut b = [0u8; 16];
    for (i, byte) in b.iter_mut().enumerate().take(6) {
        *byte = ((stamp >> (8 * (5 - i))) & 0xff) as u8;
    }
    let rand_a = state.1 & 0x0fff;
    b[6] = 0x70 | (rand_a >> 8) as u8;
    b[7] = (rand_a & 0xff) as u8;
    let tail = random_bytes::<8>();
    b[8] = 0x80 | (tail[0] & 0x3f);
    b[9..16].copy_from_slice(&tail[1..8]);
    format_uuid(&b)
}

/// Lowercase random UUID (v4), used for `store_id`.
pub fn uuid_v4() -> String {
    let mut b = random_bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format_uuid(&b)
}

/// 32 random bytes as hex: a client token (only its hash is stored).
pub(crate) fn token_hex() -> String {
    hex(&random_bytes::<32>())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v7_is_monotonic_and_well_formed() {
        let a = uuid_v7(Some(1_800_000_000_000));
        let b = uuid_v7(Some(1_800_000_000_000));
        assert!(a < b, "{a} {b}");
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "7");
        assert_eq!(sha256_hex(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
