//! Hyperion Raw Ingress — production raw binary gateway for Hyperion Mesh.
//!
//! Listens on TCP+UDP 127.0.0.1:8081 (or HYPERION_RAW_ADDR), validates PoPP chain
//! blocks, executes atomic JIT-DEX swaps in a lock-free RAMAN ledger, and
//! asynchronously persists a WAL to `traces/wal.jsonl` for replay into Supabase.
//!
//! Two operating modes:
//!   - PoPP mode (default): clients send 192-byte frames (128-byte chain block +
//!     64-byte swap) and the gateway validates them.
//!   - Bank mode (HYPERION_RAW_REQUIRE_POPP=false): clients send only the 64-byte
//!     swap payload; the gateway mints the chain block from server-side jitter, so
//!     no LoRA or user-side hardware is required.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use radnet_morphic_kernel::blockchain::{
    build_chain_block_128, derive_physical_id, raman_anomaly::RamanAnomalyDetector,
    validate_chain_block_128, validate_chain_link, JitterStats, RamanLedger,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

const FRAME_LEN: usize = 192; // 128 bytes chain block + 64 bytes swap
const CHAIN_BLOCK_LEN: usize = 128;
const SWAP_LEN: usize = 64; // raw swap payload used in bank mode (no client PoPP block)

const OK: u8 = 0x00;
const INVALID_BLOCK: u8 = 0x01;
const INSUFFICIENT_FUNDS: u8 = 0x02;
const CHAIN_LINK_BROKEN: u8 = 0x03;

/// Write-ahead log entry.  Flushed asynchronously to a JSONL file; later replayed
/// into Supabase so the hot nanosecond path never blocks on a database round-trip.
#[derive(Serialize, Clone)]
struct TxLog {
    timestamp_ns: u64,
    src: u16,
    dst: u16,
    amount: u64,
    eurc: u64,
    fx_rate: u64,
    status: u8,
    jitter_samples: u32,
    jitter_mean_ns: u32,
    jitter_std_ns: u32,
    jitter_min_ns: u32,
    jitter_max_ns: u32,
    physical_id: String,
    block_hash: String,
    server_latency_ns: u64,
    raman_anomaly_z: f64,
    raman_is_anomaly: bool,
}

struct Session {
    ledger: Arc<RamanLedger>,
    expected_prev: [u8; 32],
    processed: AtomicU64,
}

#[derive(Clone, Copy)]
struct SchedulingProfile {
    profile: &'static str,
}

#[cfg(windows)]
fn apply_raman_priority_profile() -> SchedulingProfile {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
        HIGH_PRIORITY_CLASS, REALTIME_PRIORITY_CLASS, THREAD_PRIORITY_HIGHEST,
        THREAD_PRIORITY_TIME_CRITICAL,
    };

    let mut profile = "best_effort_default";
    unsafe {
        let process = GetCurrentProcess();
        let thread = GetCurrentThread();
        let process_rt = SetPriorityClass(process, REALTIME_PRIORITY_CLASS) != 0;
        let thread_rt = SetThreadPriority(thread, THREAD_PRIORITY_TIME_CRITICAL) != 0;
        if process_rt && thread_rt {
            profile = "realtime_process + time_critical_thread";
        } else {
            let process_high = SetPriorityClass(process, HIGH_PRIORITY_CLASS) != 0;
            let thread_high = SetThreadPriority(thread, THREAD_PRIORITY_HIGHEST) != 0;
            if process_high || thread_high {
                profile = "high_process + highest_thread";
            }
        }
    }
    SchedulingProfile { profile }
}

#[cfg(not(windows))]
fn apply_raman_priority_profile() -> SchedulingProfile {
    SchedulingProfile {
        profile: "best_effort_default",
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

const BANK_KNOWLEDGE_Q: u16 = 1000;
const BANK_SPEED_NS: u16 = 620;
const BANK_PHI: &str = "HyperionBank";
const JITTER_SAMPLES: usize = 256;

/// Sample high-resolution timer deltas.  Each sample yields the OS-scheduler/timer jitter,
/// which replaces LoRA as the physical entropy source in bank deployments without user devices.
fn sample_jitter_deltas(count: usize) -> Vec<u64> {
    let mut deltas = Vec::with_capacity(count);
    let mut last = Instant::now();
    for _ in 0..count {
        std::thread::yield_now();
        let now = Instant::now();
        deltas.push((now - last).as_nanos() as u64);
        last = now;
    }
    deltas
}

fn jitter_stats_from_deltas(deltas: &[u64]) -> JitterStats {
    if deltas.is_empty() {
        return JitterStats::default();
    }
    let n = deltas.len() as u64;
    let min = *deltas.iter().min().unwrap();
    let max = *deltas.iter().max().unwrap();
    let sum = deltas.iter().map(|d| *d as u128).sum::<u128>();
    let mean = (sum / n as u128) as u64;
    let sum_sq = deltas
        .iter()
        .map(|d| {
            let diff = (*d as i128) - (mean as i128);
            (diff * diff) as u128
        })
        .sum::<u128>();
    let variance = sum_sq / n as u128;
    let std = (variance as f64).sqrt() as u64;
    JitterStats {
        samples: n as u32,
        mean_ns: mean as u32,
        std_ns: std as u32,
        min_ns: min as u32,
        max_ns: max as u32,
    }
}

struct AtomicJitterStats {
    samples: AtomicU32,
    mean_ns: AtomicU32,
    std_ns: AtomicU32,
    min_ns: AtomicU32,
    max_ns: AtomicU32,
}

impl AtomicJitterStats {
    fn load(&self) -> JitterStats {
        JitterStats {
            samples: self.samples.load(Ordering::Relaxed),
            mean_ns: self.mean_ns.load(Ordering::Relaxed),
            std_ns: self.std_ns.load(Ordering::Relaxed),
            min_ns: self.min_ns.load(Ordering::Relaxed),
            max_ns: self.max_ns.load(Ordering::Relaxed),
        }
    }

    fn store(&self, stats: JitterStats) {
        self.samples.store(stats.samples, Ordering::Relaxed);
        self.mean_ns.store(stats.mean_ns, Ordering::Relaxed);
        self.std_ns.store(stats.std_ns, Ordering::Relaxed);
        self.min_ns.store(stats.min_ns, Ordering::Relaxed);
        self.max_ns.store(stats.max_ns, Ordering::Relaxed);
    }
}

fn spawn_jitter_sampler() -> Arc<AtomicJitterStats> {
    let stats = Arc::new(AtomicJitterStats {
        samples: AtomicU32::new(0),
        mean_ns: AtomicU32::new(0),
        std_ns: AtomicU32::new(0),
        min_ns: AtomicU32::new(0),
        max_ns: AtomicU32::new(0),
    });
    let stats_clone = stats.clone();
    std::thread::spawn(move || loop {
        let new_stats = jitter_stats_from_deltas(&sample_jitter_deltas(JITTER_SAMPLES));
        stats_clone.store(new_stats);
        std::thread::sleep(Duration::from_millis(50));
    });
    stats
}

fn load_bank_plka_key() -> [u8; 32] {
    let hex = std::env::var("HYPERION_BANK_PLKA_KEY").unwrap_or_default();
    if hex.len() == 64 {
        if let Ok(bytes) = hex::decode(&hex) {
            let mut out = [0u8; 32];
            out.copy_from_slice(&bytes);
            println!("[HYPERION_RAW] bank PLKA key loaded");
            return out;
        }
    }
    if !hex.is_empty() {
        eprintln!("[HYPERION_RAW] HYPERION_BANK_PLKA_KEY must be 64 hex chars; using zeros");
    } else {
        println!("[HYPERION_RAW] HYPERION_BANK_PLKA_KEY not set; bank-mode physical_id will be all-zeros (dev only)");
    }
    [0u8; 32]
}

fn require_popp() -> bool {
    std::env::var("HYPERION_RAW_REQUIRE_POPP")
        .unwrap_or_else(|_| "true".to_string())
        .to_lowercase()
        != "false"
}

const WAL_BATCH_SIZE: usize = 64;
const WAL_CHANNEL_BOUND: usize = 1024;

fn flush_wal_batch(writer: &mut std::io::LineWriter<std::fs::File>, batch: &mut Vec<TxLog>) {
    if batch.is_empty() {
        return;
    }
    for tx in batch.drain(..) {
        if let Ok(line) = serde_json::to_string(&tx) {
            let _ = writeln!(writer, "{}", line);
        }
    }
    let _ = writer.flush();
}

fn spawn_wal_logger(rx: Receiver<TxLog>) {
    std::thread::spawn(move || {
        std::fs::create_dir_all("traces").ok();
        let path = "traces/wal.jsonl";
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(f) => {
                let mut writer = std::io::LineWriter::new(f);
                let mut batch = Vec::with_capacity(WAL_BATCH_SIZE);
                loop {
                    match rx.recv() {
                        Ok(tx) => {
                            batch.push(tx);
                            if batch.len() >= WAL_BATCH_SIZE {
                                flush_wal_batch(&mut writer, &mut batch);
                            }
                        }
                        Err(_) => {
                            flush_wal_batch(&mut writer, &mut batch);
                            break;
                        }
                    }
                }
            }
            Err(e) => eprintln!("[HYPERION_RAW] WAL logger failed to open file: {e}"),
        }
    });
}

/// Optional background DEX listener.  If ALCHEMY_RPC_URL is set, the gateway polls
/// eth_blockNumber every 10 seconds to prove RPC connectivity without touching the
/// hot authorization loop.
fn spawn_alchemy_listener() {
    let rpc = std::env::var("ALCHEMY_RPC_URL").unwrap_or_default();
    if rpc.is_empty() {
        println!("[HYPERION_RAW] ALCHEMY_RPC_URL not set; DEX listener disabled");
        return;
    }
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::new();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_blockNumber",
            "params": []
        });
        loop {
            match client
                .post(&rpc)
                .json(&body)
                .timeout(std::time::Duration::from_secs(10))
                .send()
            {
                Ok(resp) => {
                    if let Ok(text) = resp.text() {
                        println!("[HYPERION_RAW] Alchemy blockNumber: {text}");
                    }
                }
                Err(e) => eprintln!("[HYPERION_RAW] Alchemy poll failed: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

fn run_udp(
    socket: UdpSocket,
    ledger: Arc<RamanLedger>,
    log_tx: SyncSender<TxLog>,
    jitter: Arc<AtomicJitterStats>,
    bank_key: [u8; 32],
    require_popp: bool,
    anomaly: Arc<std::sync::Mutex<RamanAnomalyDetector>>,
) {
    let mut buf = [0u8; FRAME_LEN];
    let mut session = Session {
        ledger,
        expected_prev: [0u8; 32],
        processed: AtomicU64::new(0),
    };
    loop {
        let start = Instant::now();
        let (len, peer) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[HYPERION_RAW] udp recv error: {e}");
                continue;
            }
        };
        let server_jitter = jitter.load();
        let status = if require_popp {
            if len != FRAME_LEN {
                continue;
            }
            process_frame(&buf, &mut session, &log_tx, &server_jitter, start, &anomaly)
        } else {
            if len != SWAP_LEN {
                continue;
            }
            let swap: &[u8; SWAP_LEN] = match buf[..SWAP_LEN].try_into() {
                Ok(s) => s,
                Err(_) => continue,
            };
            process_bank_frame(
                swap,
                &mut session,
                &log_tx,
                &server_jitter,
                &bank_key,
                start,
                &anomaly,
            )
        };
        let _ = socket.send_to(&[status], peer);
    }
}

fn process_frame(
    frame: &[u8; FRAME_LEN],
    session: &mut Session,
    log_tx: &SyncSender<TxLog>,
    server_jitter: &JitterStats,
    start: Instant,
    anomaly: &Arc<std::sync::Mutex<RamanAnomalyDetector>>,
) -> u8 {
    let block_bytes = &frame[..CHAIN_BLOCK_LEN];
    let block: &[u8; CHAIN_BLOCK_LEN] = match block_bytes.try_into() {
        Ok(b) => b,
        Err(_) => return INVALID_BLOCK,
    };

    if !validate_chain_block_128(block) {
        return INVALID_BLOCK;
    }
    if !validate_chain_link(&session.expected_prev, block) {
        return CHAIN_LINK_BROKEN;
    }

    let swap = &frame[CHAIN_BLOCK_LEN..];
    let src = u16::from_le_bytes([swap[0], swap[1]]);
    let dst = u16::from_le_bytes([swap[2], swap[3]]);
    let amount = u64::from_le_bytes(match swap[4..12].try_into() {
        Ok(v) => v,
        Err(_) => return INVALID_BLOCK,
    });
    let fx_rate = u64::from_le_bytes(match swap[12..20].try_into() {
        Ok(v) => v,
        Err(_) => return INVALID_BLOCK,
    });

    let physical_id: [u8; 16] = block[16..32].try_into().unwrap_or([0u8; 16]);
    match session
        .ledger
        .execute_jit_dex_swap(src, dst, amount, fx_rate)
    {
        Ok(eurc) => {
            let mut h = Sha256::new();
            h.update(block_bytes);
            let block_hash: [u8; 32] = h.finalize().into();
            let elapsed_ns = start.elapsed().as_nanos() as u64;
            let (anomaly_z, is_anomaly) = {
                let mut guard = anomaly.lock().unwrap_or_else(|p| p.into_inner());
                guard.update(elapsed_ns)
            };
            let _ = log_tx.send(TxLog {
                timestamp_ns: now_ns(),
                src,
                dst,
                amount,
                eurc,
                fx_rate,
                status: OK,
                jitter_samples: server_jitter.samples,
                jitter_mean_ns: server_jitter.mean_ns,
                jitter_std_ns: server_jitter.std_ns,
                jitter_min_ns: server_jitter.min_ns,
                jitter_max_ns: server_jitter.max_ns,
                physical_id: hex::encode(physical_id),
                block_hash: hex::encode(block_hash),
                server_latency_ns: elapsed_ns,
                raman_anomaly_z: anomaly_z,
                raman_is_anomaly: is_anomaly,
            });
            session.expected_prev.copy_from_slice(&block_hash);
            session.processed.fetch_add(1, Ordering::Relaxed);
            OK
        }
        Err(_) => INSUFFICIENT_FUNDS,
    }
}

/// Bank mode: the gateway itself mints the PoPP block from server-side jitter.
/// No LoRA or user-side hardware is required.
fn process_bank_frame(
    swap: &[u8; SWAP_LEN],
    session: &mut Session,
    log_tx: &SyncSender<TxLog>,
    server_jitter: &JitterStats,
    bank_key: &[u8; 32],
    start: Instant,
    anomaly: &Arc<std::sync::Mutex<RamanAnomalyDetector>>,
) -> u8 {
    let src = u16::from_le_bytes([swap[0], swap[1]]);
    let dst = u16::from_le_bytes([swap[2], swap[3]]);
    let amount = u64::from_le_bytes(match swap[4..12].try_into() {
        Ok(v) => v,
        Err(_) => return INVALID_BLOCK,
    });
    let fx_rate = u64::from_le_bytes(match swap[12..20].try_into() {
        Ok(v) => v,
        Err(_) => return INVALID_BLOCK,
    });

    let index = (session.processed.load(Ordering::Relaxed) % 256) as u8;
    let deltas: [u64; 4] = [
        server_jitter.mean_ns as u64,
        server_jitter.std_ns as u64,
        server_jitter.min_ns as u64,
        server_jitter.max_ns as u64,
    ];
    let (physical_id, entropy_hash) = derive_physical_id(bank_key, "server", "bank", 0, &deltas);
    let block = build_chain_block_128(
        index,
        &physical_id,
        BANK_PHI,
        entropy_hash,
        BANK_KNOWLEDGE_Q,
        BANK_SPEED_NS,
        &session.expected_prev,
        server_jitter,
    );
    let mut h = Sha256::new();
    h.update(&block);
    let block_hash: [u8; 32] = h.finalize().into();

    match session
        .ledger
        .execute_jit_dex_swap(src, dst, amount, fx_rate)
    {
        Ok(eurc) => {
            let elapsed_ns = start.elapsed().as_nanos() as u64;
            let (anomaly_z, is_anomaly) = {
                let mut guard = anomaly.lock().unwrap_or_else(|p| p.into_inner());
                guard.update(elapsed_ns)
            };
            let _ = log_tx.send(TxLog {
                timestamp_ns: now_ns(),
                src,
                dst,
                amount,
                eurc,
                fx_rate,
                status: OK,
                jitter_samples: server_jitter.samples,
                jitter_mean_ns: server_jitter.mean_ns,
                jitter_std_ns: server_jitter.std_ns,
                jitter_min_ns: server_jitter.min_ns,
                jitter_max_ns: server_jitter.max_ns,
                physical_id: hex::encode(physical_id),
                block_hash: hex::encode(block_hash),
                server_latency_ns: elapsed_ns,
                raman_anomaly_z: anomaly_z,
                raman_is_anomaly: is_anomaly,
            });
            session.expected_prev.copy_from_slice(&block_hash);
            session.processed.fetch_add(1, Ordering::Relaxed);
            OK
        }
        Err(_) => INSUFFICIENT_FUNDS,
    }
}

fn handle_client(
    mut socket: TcpStream,
    ledger: Arc<RamanLedger>,
    log_tx: SyncSender<TxLog>,
    jitter: Arc<AtomicJitterStats>,
    bank_key: [u8; 32],
    require_popp: bool,
    anomaly: Arc<std::sync::Mutex<RamanAnomalyDetector>>,
) {
    let _ = socket.set_nodelay(true);
    let mut session = Session {
        ledger,
        expected_prev: [0u8; 32],
        processed: AtomicU64::new(0),
    };

    loop {
        let start = Instant::now();
        let server_jitter = jitter.load();
        let status = if require_popp {
            let mut buf = [0u8; FRAME_LEN];
            match socket.read_exact(&mut buf) {
                Ok(()) => {
                    process_frame(&buf, &mut session, &log_tx, &server_jitter, start, &anomaly)
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    eprintln!("[HYPERION_RAW] read failed: {e}");
                    break;
                }
            }
        } else {
            let mut buf = [0u8; SWAP_LEN];
            match socket.read_exact(&mut buf) {
                Ok(()) => process_bank_frame(
                    &buf,
                    &mut session,
                    &log_tx,
                    &server_jitter,
                    &bank_key,
                    start,
                    &anomaly,
                ),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    eprintln!("[HYPERION_RAW] read failed: {e}");
                    break;
                }
            }
        };
        if socket.write_all(&[status]).is_err() {
            break;
        }
    }
}

fn main() {
    let scheduling_profile = apply_raman_priority_profile();
    println!(
        "[HYPERION_RAW] RAMAN scheduling profile: {}",
        scheduling_profile.profile
    );

    let addr = std::env::var("HYPERION_RAW_ADDR").unwrap_or_else(|_| "127.0.0.1:8081".to_string());
    let db_url = std::env::var("DATABASE_URL").unwrap_or_default();
    if db_url.is_empty() {
        println!("[HYPERION_RAW] DATABASE_URL not set; WAL will be written to traces/wal.jsonl");
    } else {
        let masked = db_url.replace("://", "://***@");
        println!("[HYPERION_RAW] DATABASE_URL configured: {masked}");
    }

    let ledger = Arc::new(RamanLedger::new(1024, 1_000_000_000_000_000, 0));
    let (log_tx, log_rx) = sync_channel::<TxLog>(WAL_CHANNEL_BOUND);
    spawn_wal_logger(log_rx);
    spawn_alchemy_listener();

    let jitter = spawn_jitter_sampler();
    let bank_key = load_bank_plka_key();
    let require_popp = require_popp();
    let anomaly = Arc::new(std::sync::Mutex::new(RamanAnomalyDetector::new()));
    println!(
        "[HYPERION_RAW] operating mode: {}",
        if require_popp {
            "PoPP (client chain blocks required)"
        } else {
            "BANK (server-side jitter, no user devices)"
        }
    );

    let udp_socket = UdpSocket::bind(&addr).expect(&format!("bind udp {addr}"));
    let listener = TcpListener::bind(&addr).expect(&format!("bind tcp {addr}"));
    println!("[HYPERION_RAW] TCP+UDP listening on {addr} (sync, nodelay)");
    if require_popp {
        println!("[HYPERION_RAW] frame layout: 128 bytes chain block + 64 bytes swap payload");
    } else {
        println!("[HYPERION_RAW] bank-mode frame layout: 64 bytes swap payload only");
    }

    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8);
    println!("[HYPERION_RAW] TCP worker pool: {worker_count} threads");
    for _ in 0..worker_count {
        let worker_listener = listener
            .try_clone()
            .expect("[HYPERION_RAW] failed to clone TCP listener");
        let ledger = ledger.clone();
        let log_tx = log_tx.clone();
        let jitter = jitter.clone();
        let bank_key = bank_key;
        let anomaly = anomaly.clone();
        std::thread::spawn(move || {
            for stream in worker_listener.incoming() {
                match stream {
                    Ok(s) => handle_client(
                        s,
                        ledger.clone(),
                        log_tx.clone(),
                        jitter.clone(),
                        bank_key,
                        require_popp,
                        anomaly.clone(),
                    ),
                    Err(e) => eprintln!("[HYPERION_RAW] accept failed: {e}"),
                }
            }
        });
    }

    let udp_ledger = ledger.clone();
    let udp_log_tx = log_tx.clone();
    let udp_jitter = jitter.clone();
    let udp_bank_key = bank_key;
    let udp_anomaly = anomaly.clone();
    std::thread::spawn(move || {
        run_udp(
            udp_socket,
            udp_ledger,
            udp_log_tx,
            udp_jitter,
            udp_bank_key,
            require_popp,
            udp_anomaly,
        )
    });
}
