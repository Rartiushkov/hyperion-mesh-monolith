//! Sovereign PoPP blockchain core — native Rust port of the Python tooling.
//!
//! Sources:
//! - `tools/mint_genesis_multi.py` -> `_build_block()` and `_derive_physical_id()`
//! - `tools/mint_resonance_chain.py` -> `_build_chain_block_128()` and chain validation
//! - `tools/keygen/silent_secure_keygen.py` -> `privacy_amplify()`

#![cfg(feature = "std")]

pub mod raman_anomaly;

use std::time::{SystemTime, UNIX_EPOCH};

use blake2::{Blake2b512, Blake2s256, Digest as Blake2Digest};
use sha2::Sha256;

pub const BLOCK_SIZE_192: usize = 192;
pub const BLOCK_SIZE_128: usize = 128;

pub const BLOCK_MAGIC_192: &[u8; 8] = b"GRP3MULI";
pub const BLOCK_MAGIC_128: &[u8; 8] = b"GRESCHN1";

/// Jitter statistics embedded in a PoPP block.
#[derive(Debug, Clone, Copy, Default)]
pub struct JitterStats {
    pub samples: u32,
    pub mean_ns: u32,
    pub std_ns: u32,
    pub min_ns: u32,
    pub max_ns: u32,
}

/// Source id mapping that matches the Python `SOURCE_MAP` order.
/// lora=0, audio=1, wifi=2, accel=3, plc=4, manual=5
pub fn source_id(source: &str) -> u8 {
    match source {
        "lora" => 0,
        "audio" => 1,
        "wifi" => 2,
        "accel" => 3,
        "plc" => 4,
        "manual" => 5,
        _ => 0xFF,
    }
}

/// Derive the hardware-bound physical_id and entropy_hash from a PLKA key,
/// source/port/baud metadata and a slice of jitter deltas.
///
/// Ported from `tools/mint_genesis_multi.py::_derive_physical_id()`.
pub fn derive_physical_id(
    plka_key: &[u8; 32],
    source: &str,
    port: &str,
    baud: u32,
    deltas: &[u64],
) -> ([u8; 16], u64) {
    let mut payload = Vec::with_capacity(
        plka_key.len() + source.len() + port.len() + 4 + deltas.len().min(64) * 8,
    );
    payload.extend_from_slice(plka_key);
    payload.extend_from_slice(source.as_bytes());
    payload.extend_from_slice(port.as_bytes());
    payload.extend_from_slice(&baud.to_le_bytes());
    for d in deltas.iter().take(64) {
        payload.extend_from_slice(&d.to_le_bytes());
    }

    let mut hasher_s = Blake2s256::new();
    hasher_s.update(&payload);
    let pid = hasher_s.finalize();
    let physical_id: [u8; 16] = pid[..16].try_into().expect("32-byte digest");

    let mut hasher_b = Blake2b512::new();
    hasher_b.update(&payload);
    let ehash = hasher_b.finalize();
    let entropy_hash = u64::from_le_bytes(ehash[..8].try_into().expect("64-byte digest"));

    (physical_id, entropy_hash)
}

/// Build a 192-byte PoPP v3 multi-source genesis block.
///
/// Ported from `tools/mint_genesis_multi.py::_build_block()`.
#[allow(clippy::too_many_arguments)]
pub fn build_block_192(
    version: u8,
    source_id: u8,
    physical_id: &[u8; 16],
    plka_sha256: &[u8; 32],
    phi: &str,
    entropy_hash: u64,
    knowledge_q: u16,
    speed_ns: u16,
    jitter: &JitterStats,
    source_meta: &str,
) -> [u8; BLOCK_SIZE_192] {
    let mut out = [0u8; BLOCK_SIZE_192];
    out[0..8].copy_from_slice(BLOCK_MAGIC_192);
    out[8] = version;
    out[9] = 0x03; // PoPP + MultiSource flags
    out[10..12].copy_from_slice(&knowledge_q.to_le_bytes());
    out[12..14].copy_from_slice(&speed_ns.to_le_bytes());
    out[14] = source_id;
    out[15] = 0x00;
    out[16..32].copy_from_slice(&physical_id[..]);

    let phi_b = truncate_ascii(phi, 16);
    out[32..32 + phi_b.len()].copy_from_slice(&phi_b);
    out[48..64].copy_from_slice(&plka_sha256[..16]);

    out[64..72].copy_from_slice(&entropy_hash.to_le_bytes());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    out[72..76].copy_from_slice(&now.to_le_bytes());
    out[76..80].copy_from_slice(&jitter.samples.to_le_bytes());
    out[80..84].copy_from_slice(&jitter.mean_ns.to_le_bytes());
    out[84..88].copy_from_slice(&jitter.std_ns.to_le_bytes());
    out[88..92].copy_from_slice(&jitter.min_ns.to_le_bytes());
    out[92..96].copy_from_slice(&jitter.max_ns.to_le_bytes());

    let meta = truncate_bytes(source_meta.as_bytes(), 60);
    out[96..96 + meta.len()].copy_from_slice(&meta);

    let seal = blake2s_32(&out[..160]);
    out[160..192].copy_from_slice(&seal);

    out
}

/// Build a 128-byte chain-linked PoPP block.
///
/// Ported from `tools/mint_resonance_chain.py::_build_chain_block_128()`.
#[allow(clippy::too_many_arguments)]
pub fn build_chain_block_128(
    block_index: u8,
    physical_id: &[u8; 16],
    phi_text: &str,
    entropy_hash: u64,
    knowledge_q: u16,
    speed_ns: u16,
    prev_hash32: &[u8; 32],
    jitter: &JitterStats,
) -> [u8; BLOCK_SIZE_128] {
    let mut out = [0u8; BLOCK_SIZE_128];
    out[0..8].copy_from_slice(BLOCK_MAGIC_128);
    out[8] = 1; // version
    out[9] = block_index;
    out[10..12].copy_from_slice(&knowledge_q.to_le_bytes());
    out[12..14].copy_from_slice(&speed_ns.to_le_bytes());
    out[14..16].copy_from_slice(&[0u8; 2]);
    out[16..32].copy_from_slice(&physical_id[..]);

    let phi = truncate_ascii(phi_text, 32);
    out[32..32 + phi.len()].copy_from_slice(&phi);
    out[32 + phi.len()..64].fill(0);

    out[64..96].copy_from_slice(&prev_hash32[..]);
    out[96..104].copy_from_slice(&entropy_hash.to_le_bytes());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    out[104..108].copy_from_slice(&now.to_le_bytes());
    out[108..112].copy_from_slice(&jitter.mean_ns.to_le_bytes());
    out[112..116].copy_from_slice(&jitter.std_ns.to_le_bytes());
    out[116..120].copy_from_slice(&jitter.samples.to_le_bytes());

    let seal = blake2s_8(&out[..120]);
    out[120..128].copy_from_slice(&seal);

    out
}

/// Verify the internal 8-byte blake2s seal of a 128-byte chain block.
pub fn validate_chain_block_128(block: &[u8; BLOCK_SIZE_128]) -> bool {
    let expected = blake2s_8(&block[..120]);
    &block[120..128] == expected.as_slice()
}

/// Verify that `current` block correctly links to a previous 32-byte hash.
pub fn validate_chain_link(prev_hash32: &[u8; 32], current: &[u8; BLOCK_SIZE_128]) -> bool {
    &current[64..96] == prev_hash32.as_slice()
}

/// Privacy amplification: SHA256 over a bit-string of 0/1 values, optionally extended.
///
/// Ported from `tools/keygen/silent_secure_keygen.py::privacy_amplify()`.
pub fn privacy_amplify(bits: &[u8], out_bytes: usize) -> Vec<u8> {
    let payload: Vec<u8> = bits
        .iter()
        .map(|b| if *b != 0 { b'1' } else { b'0' })
        .collect();
    let digest = sha2_32(&payload);
    if out_bytes <= digest.len() {
        return digest[..out_bytes].to_vec();
    }
    let mut out = digest.to_vec();
    let mut ctr = 1u32;
    while out.len() < out_bytes {
        let mut h = Sha256::new();
        h.update(&digest);
        h.update(&ctr.to_le_bytes());
        out.extend_from_slice(&h.finalize());
        ctr += 1;
    }
    out.truncate(out_bytes);
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate_ascii(s: &str, max_len: usize) -> Vec<u8> {
    s.chars()
        .filter(|c| c.is_ascii())
        .take(max_len)
        .map(|c| c as u8)
        .collect()
}

fn truncate_bytes(b: &[u8], max_len: usize) -> Vec<u8> {
    b.iter().take(max_len).copied().collect()
}

fn blake2s_8(data: &[u8]) -> [u8; 8] {
    let mut h = Blake2s256::new();
    h.update(data);
    let full = h.finalize();
    full[..8].try_into().expect("8-byte slice")
}

fn blake2s_32(data: &[u8]) -> [u8; 32] {
    let mut h = Blake2s256::new();
    h.update(data);
    h.finalize().into()
}

fn sha2_32(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

pub mod ledger;
pub use ledger::*;
