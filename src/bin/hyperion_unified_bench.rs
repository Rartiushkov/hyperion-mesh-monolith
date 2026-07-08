//! Hyperion Unified Bench — 100 000 транзакций через сырой TCP-порт 8081.
//!
//! Каждая транзакция = 192-байтный бинарный кадр (128 chain block + 64 swap payload).
//! Замеряется полная round-trip latency с учётом транспортного оверхеда.

use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use radnet_morphic_kernel::blockchain::{build_chain_block_128, JitterStats};
use sha2::{Digest, Sha256};

const FRAME_LEN: usize = 192;
const CHAIN_BLOCK_LEN: usize = 128;
const SWAP_LEN: usize = 64;

fn percentile(sorted_ns: &[u64], p: f64) -> f64 {
    if sorted_ns.is_empty() {
        return 0.0;
    }
    let k = (sorted_ns.len() - 1) as f64 * p / 100.0;
    let f = k as usize;
    let c = (f + 1).min(sorted_ns.len() - 1);
    if f == c {
        sorted_ns[f] as f64
    } else {
        sorted_ns[f] as f64 + (sorted_ns[c] as f64 - sorted_ns[f] as f64) * (k - f as f64)
    }
}

fn stddev(ns: &[u64], mean: f64) -> f64 {
    if ns.len() < 2 {
        return 0.0;
    }
    let var = ns.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / (ns.len() - 1) as f64;
    var.sqrt()
}

fn build_chain_frames(n: usize) -> Vec<Vec<u8>> {
    let pid = [0u8; 16];
    let jitter = JitterStats {
        samples: 64,
        mean_ns: 1234,
        std_ns: 567,
        min_ns: 100,
        max_ns: 9999,
    };
    let mut prev_hash = [0u8; 32];
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(n);

    for i in 0..n {
        let block = build_chain_block_128(
            (i & 0xFF) as u8,
            &pid,
            "RadCoin",
            0,
            1234,
            620,
            &prev_hash,
            &jitter,
        );
        let mut frame = [0u8; FRAME_LEN];
        frame[..CHAIN_BLOCK_LEN].copy_from_slice(&block);

        // Swap payload: src(u16) dst(u16) amount(u64) fx_rate(u64).
        frame[CHAIN_BLOCK_LEN..CHAIN_BLOCK_LEN + 2].copy_from_slice(&0u16.to_le_bytes());
        frame[CHAIN_BLOCK_LEN + 2..CHAIN_BLOCK_LEN + 4].copy_from_slice(&1u16.to_le_bytes());
        frame[CHAIN_BLOCK_LEN + 4..CHAIN_BLOCK_LEN + 12]
            .copy_from_slice(&1_000_000u64.to_le_bytes());
        frame[CHAIN_BLOCK_LEN + 12..CHAIN_BLOCK_LEN + 20]
            .copy_from_slice(&1_085_000u64.to_le_bytes());

        frames.push(frame.to_vec());

        // Next block links to sha256 of this chain block.
        let mut h = Sha256::new();
        h.update(&block);
        prev_hash.copy_from_slice(&h.finalize());
    }
    frames
}

/// Bank-mode payloads: only the 64-byte swap; the server mints the PoPP block from jitter.
fn build_swap_payloads(n: usize) -> Vec<Vec<u8>> {
    let mut payloads: Vec<Vec<u8>> = Vec::with_capacity(n);
    for _ in 0..n {
        let mut p = vec![0u8; SWAP_LEN];
        p[0..2].copy_from_slice(&0u16.to_le_bytes());
        p[2..4].copy_from_slice(&1u16.to_le_bytes());
        p[4..12].copy_from_slice(&1_000_000u64.to_le_bytes());
        p[12..20].copy_from_slice(&1_085_000u64.to_le_bytes());
        payloads.push(p);
    }
    payloads
}

fn run_tcp(host: &str, payloads: &[Vec<u8>]) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(host)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut latencies = Vec::with_capacity(payloads.len());
    let mut ack = [0u8; 1];
    for (idx, payload) in payloads.iter().enumerate() {
        let start = Instant::now();
        stream.write_all(payload)?;
        stream.read_exact(&mut ack)?;
        latencies.push(start.elapsed().as_nanos() as u64);
        if ack[0] != 0x00 {
            eprintln!(
                "[HYPERION_UNIFIED_BENCH] payload {idx} returned status {:#04x}",
                ack[0]
            );
        }
        if idx > 0 && idx % 10_000 == 0 {
            println!("[HYPERION_UNIFIED_BENCH] processed {idx} payloads");
        }
    }
    Ok(latencies)
}

fn run_udp(host: &str, payloads: &[Vec<u8>]) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(30)))?;
    socket.connect(host)?;
    let mut latencies = Vec::with_capacity(payloads.len());
    let mut ack = [0u8; 1];
    for (idx, payload) in payloads.iter().enumerate() {
        let start = Instant::now();
        socket.send(payload)?;
        socket.recv(&mut ack)?;
        latencies.push(start.elapsed().as_nanos() as u64);
        if ack[0] != 0x00 {
            eprintln!(
                "[HYPERION_UNIFIED_BENCH] payload {idx} returned status {:#04x}",
                ack[0]
            );
        }
        if idx > 0 && idx % 10_000 == 0 {
            println!("[HYPERION_UNIFIED_BENCH] processed {idx} payloads");
        }
    }
    Ok(latencies)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let events = args
        .windows(2)
        .find(|w| w[0] == "--events")
        .and_then(|w| w[1].parse::<usize>().ok())
        .unwrap_or(100_000);
    let use_udp = args.contains(&"--udp".to_string());
    let bank_mode = args.contains(&"--bank".to_string());
    let host = "127.0.0.1:8081";
    let proto = if use_udp { "UDP" } else { "TCP" };
    let mode = if bank_mode { "BANK" } else { "PoPP" };

    println!("[HYPERION_UNIFIED_BENCH] generating {events} {mode} payloads...");
    let payloads = if bank_mode {
        build_swap_payloads(events)
    } else {
        build_chain_frames(events)
    };

    println!("[HYPERION_UNIFIED_BENCH] connecting to {host} over {proto} ({mode})...");
    let wall_start = Instant::now();
    let mut latencies_ns = if use_udp {
        run_udp(host, &payloads)?
    } else {
        run_tcp(host, &payloads)?
    };
    let wall_total = wall_start.elapsed();

    latencies_ns.sort_unstable();
    let mean_ns = latencies_ns.iter().sum::<u64>() as f64 / latencies_ns.len() as f64;
    let p50_ns = percentile(&latencies_ns, 50.0);
    let p99_ns = percentile(&latencies_ns, 99.0);
    let std_ns = stddev(&latencies_ns, mean_ns);
    let min_ns = latencies_ns.first().copied().unwrap_or(0);
    let max_ns = latencies_ns.last().copied().unwrap_or(0);
    let rps = 1_000_000_000.0 / mean_ns;
    let throughput_total = events as f64 / wall_total.as_secs_f64();

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let verdict = if p99_ns < 50_000.0 {
        "TRANSPORT_BARRIER_BROKEN_UNDER_50us"
    } else {
        "NEEDS_TUNING"
    };
    let report = serde_json::json!({
        "benchmark": "hyperion_unified_bench",
        "mode": mode,
        "timestamp_epoch_s": ts,
        "events": events,
        "host": host,
        "transport": proto,
        "latency_ns": {
            "mean": mean_ns,
            "p50": p50_ns,
            "p99": p99_ns,
            "std": std_ns,
            "min": min_ns,
            "max": max_ns,
        },
        "rps": rps,
        "throughput_total_s": throughput_total,
        "wall_total_ms": wall_total.as_millis(),
        "verdict": verdict,
    });

    std::fs::create_dir_all("traces")?;
    let path = "traces/hyperion_unified_bench_latest.json";
    std::fs::write(path, serde_json::to_string_pretty(&report)?)?;

    println!("\n================================================================");
    println!("HYPERION UNIFIED BENCH — {events} RAW {proto} {mode} TRANSACTIONS");
    println!("================================================================");
    println!("Latency (ns):  mean={mean_ns:.1}  p50={p50_ns:.1}  p99={p99_ns:.1}");
    println!("               std={std_ns:.1}  min={min_ns}  max={max_ns}");
    println!("RPS (mean)    : {rps:.1}");
    println!("Throughput    : {throughput_total:.1} tx/s (wall)");
    println!("Verdict       : {verdict}");
    println!("Trace         : {path}");
    println!("================================================================");

    Ok(())
}
