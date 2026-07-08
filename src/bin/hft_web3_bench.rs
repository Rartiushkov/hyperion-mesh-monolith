use core_affinity::CoreId;
use futures_util::{SinkExt, StreamExt};
use radnet_morphic_kernel::{compute_budget_metrics, twin_alignment_metrics};
use secp256k1::{ecdsa::RecoverableSignature, Message, PublicKey, Secp256k1, SecretKey};
use sha3::{Digest, Keccak256};
use std::cmp;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, System};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

const DEFAULT_EVENTS: usize = 100_000;
const DEFAULT_WARMUP_EVENTS: usize = 8_192;
const DEFAULT_WS_URL: &str = "wss://base-rpc.publicnode.com";
const DEFAULT_LIVE_SMOKE_SECONDS: u64 = 15;
const CHAIN_ID_BASE: u64 = 8453;
const PRIVATE_KEY_HEX: &str = "4c0883a69102937d6231471b5dbb6204fe512961708279f0f95b9f8d0d4c1f7a";

#[derive(Clone, Copy)]
enum ParserMode {
    Fast,
    Serde,
    RamanBinary,
}

impl ParserMode {
    fn as_str(self) -> &'static str {
        match self {
            ParserMode::Fast => "fast_scan",
            ParserMode::Serde => "serde_json",
            ParserMode::RamanBinary => "raman_typed_binary",
        }
    }
}

#[derive(Clone)]
struct Config {
    events: usize,
    warmup_events: usize,
    label: String,
    ws_url: String,
    live_smoke_seconds: u64,
    use_affinity: bool,
}

#[derive(Clone, Copy)]
struct ParsedEvent {
    coeff_a: f64,
    coeff_b: f64,
    nonce: u64,
    block_number: u64,
}

#[derive(Clone, Copy)]
struct StageSample {
    parse_ns: u64,
    math_ns: u64,
    sign_ns: u64,
    total_ns: u64,
}

#[derive(Clone)]
struct BenchMetrics {
    parser: &'static str,
    samples: Vec<StageSample>,
    peak_memory_mb: f64,
    trigger_count: usize,
    raw_tx_size_bytes: usize,
}

#[derive(Clone)]
struct LiveSmokeResult {
    ws_url: String,
    requested_seconds: u64,
    connected: bool,
    disconnects: u64,
    messages: u64,
    first_message_latency_us: Option<u64>,
    subscription_mode: &'static str,
}

#[derive(Clone, Copy)]
struct SchedulingProfile {
    enabled: bool,
    profile: &'static str,
}

#[derive(serde::Deserialize)]
struct RpcEnvelope {
    params: RpcParams,
}

#[derive(serde::Deserialize)]
struct RpcParams {
    result: RpcResult,
}

#[derive(serde::Deserialize)]
struct RpcResult {
    #[serde(rename = "blockNumber")]
    block_number: String,
    nonce: String,
    coeffs: [f64; 2],
}

#[derive(Clone, Copy)]
struct SignedTrigger {
    signature: RecoverableSignature,
    hash: [u8; 32],
    event: ParsedEvent,
    spread_bps: u32,
}

struct HftSigner {
    secp: Secp256k1<secp256k1::SignOnly>,
    secret_key: SecretKey,
    to: [u8; 20],
}

impl HftSigner {
    fn new() -> Result<Self, String> {
        let sk_bytes = hex::decode(PRIVATE_KEY_HEX).map_err(|e| format!("private key hex: {e}"))?;
        let secret_key =
            SecretKey::from_slice(&sk_bytes).map_err(|e| format!("secret key parse: {e}"))?;
        Ok(Self {
            secp: Secp256k1::signing_only(),
            secret_key,
            to: [0x11; 20],
        })
    }

    fn sign_trigger(&self, event: ParsedEvent, spread_bps: u32) -> Result<SignedTrigger, String> {
        let unsigned =
            encode_legacy_tx_unsigned_fast(event.nonce, &self.to, event.block_number, spread_bps);
        let hash = keccak256(&unsigned);
        let message = Message::from_digest(hash);
        let signature = self.secp.sign_ecdsa_recoverable(&message, &self.secret_key);
        Ok(SignedTrigger {
            signature,
            hash,
            event,
            spread_bps,
        })
    }

    fn finalize_signed_tx(&self, trigger: SignedTrigger) -> Vec<u8> {
        let (recovery_id, compact) = trigger.signature.serialize_compact();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&compact[..32]);
        s.copy_from_slice(&compact[32..]);
        let v = CHAIN_ID_BASE * 2 + 35 + recovery_id.to_i32() as u64;
        let _ = trigger.hash;
        encode_legacy_tx_signed(
            trigger.event.nonce,
            1_000_000_000u64,
            220_000u64,
            &self.to,
            0u64,
            &arb_payload(trigger.event.block_number, trigger.spread_bps),
            v,
            &r,
            &s,
        )
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = parse_args()?;
    let pinned_core = if config.use_affinity {
        pin_to_first_core()
    } else {
        None
    };
    let scheduling_profile = apply_raman_priority_profile();

    let live_smoke = run_live_smoke(&config.ws_url, config.live_smoke_seconds).await;
    prime_runtime(config.warmup_events)?;
    let raman_metrics = run_bench(ParserMode::RamanBinary, config.events, config.warmup_events)?;
    let fast_metrics = run_bench(ParserMode::Fast, config.events, config.warmup_events)?;
    let serde_metrics = run_bench(ParserMode::Serde, config.events, config.warmup_events)?;
    let report = render_report(
        &config,
        pinned_core,
        scheduling_profile,
        &live_smoke,
        &raman_metrics,
        &fast_metrics,
        &serde_metrics,
    );

    let suffix = today_string();
    let label_suffix = if config.label.is_empty() {
        suffix.clone()
    } else {
        format!("{suffix}_{}", config.label)
    };
    let trace_path =
        PathBuf::from("traces").join(format!("hft_web3_benchmark_{label_suffix}.json"));
    let doc_path = PathBuf::from("docs").join(format!("HFT_WEB3_BENCHMARK_{label_suffix}.md"));
    fs::create_dir_all("traces")?;
    fs::create_dir_all("docs")?;
    fs::write(&trace_path, serde_json::to_vec_pretty(&report.to_json())?)?;
    fs::write(&doc_path, report.to_markdown())?;

    println!("[HFT_WEB3_BENCH] trace={}", trace_path.display());
    println!("[HFT_WEB3_BENCH] doc={}", doc_path.display());
    println!(
        "[HFT_WEB3_BENCH] raman_p99_total_us={:.2}",
        p99_ns(&raman_metrics.samples) as f64 / 1_000.0
    );
    println!(
        "[HFT_WEB3_BENCH] custom_p99_total_us={:.2}",
        p99_ns(&fast_metrics.samples) as f64 / 1_000.0
    );
    println!(
        "[HFT_WEB3_BENCH] baseline_p99_total_us={:.2}",
        p99_ns(&serde_metrics.samples) as f64 / 1_000.0
    );
    Ok(())
}

fn parse_args() -> Result<Config, String> {
    let mut events = DEFAULT_EVENTS;
    let mut warmup_events = DEFAULT_WARMUP_EVENTS;
    let mut label = String::from("default");
    let mut ws_url = String::from(DEFAULT_WS_URL);
    let mut live_smoke_seconds = DEFAULT_LIVE_SMOKE_SECONDS;
    let mut use_affinity = true;

    let args: Vec<String> = env::args().collect();
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--events" => {
                i += 1;
                events = args
                    .get(i)
                    .ok_or_else(|| "missing value for --events".to_string())?
                    .parse()
                    .map_err(|e| format!("--events parse: {e}"))?;
            }
            "--label" => {
                i += 1;
                label = args
                    .get(i)
                    .ok_or_else(|| "missing value for --label".to_string())?
                    .clone();
            }
            "--warmup-events" => {
                i += 1;
                warmup_events = args
                    .get(i)
                    .ok_or_else(|| "missing value for --warmup-events".to_string())?
                    .parse()
                    .map_err(|e| format!("--warmup-events parse: {e}"))?;
            }
            "--ws-url" => {
                i += 1;
                ws_url = args
                    .get(i)
                    .ok_or_else(|| "missing value for --ws-url".to_string())?
                    .clone();
            }
            "--live-smoke-seconds" => {
                i += 1;
                live_smoke_seconds = args
                    .get(i)
                    .ok_or_else(|| "missing value for --live-smoke-seconds".to_string())?
                    .parse()
                    .map_err(|e| format!("--live-smoke-seconds parse: {e}"))?;
            }
            "--no-affinity" => use_affinity = false,
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }

    Ok(Config {
        events,
        warmup_events,
        label,
        ws_url,
        live_smoke_seconds,
        use_affinity,
    })
}

fn pin_to_first_core() -> Option<CoreId> {
    let cores = core_affinity::get_core_ids()?;
    let core = cores.first().copied()?;
    core_affinity::set_for_current(core);
    Some(core)
}

#[cfg(windows)]
fn apply_raman_priority_profile() -> SchedulingProfile {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
        HIGH_PRIORITY_CLASS, REALTIME_PRIORITY_CLASS, THREAD_PRIORITY_HIGHEST,
        THREAD_PRIORITY_TIME_CRITICAL,
    };

    let mut enabled = false;
    let mut profile = "best_effort_default";
    unsafe {
        let process = GetCurrentProcess();
        let thread = GetCurrentThread();
        let process_rt = SetPriorityClass(process, REALTIME_PRIORITY_CLASS) != 0;
        let thread_rt = SetThreadPriority(thread, THREAD_PRIORITY_TIME_CRITICAL) != 0;
        if process_rt && thread_rt {
            enabled = true;
            profile = "realtime_process + time_critical_thread";
        } else {
            let process_high = SetPriorityClass(process, HIGH_PRIORITY_CLASS) != 0;
            let thread_high = SetThreadPriority(thread, THREAD_PRIORITY_HIGHEST) != 0;
            if process_high || thread_high {
                enabled = true;
                profile = "high_process + highest_thread";
            }
        }
    }

    SchedulingProfile { enabled, profile }
}

#[cfg(not(windows))]
fn apply_raman_priority_profile() -> SchedulingProfile {
    SchedulingProfile {
        enabled: false,
        profile: "best_effort_default",
    }
}

async fn run_live_smoke(ws_url: &str, seconds: u64) -> LiveSmokeResult {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut disconnects = 0u64;
    let mut messages = 0u64;
    let mut first_message_latency_us = None;
    let mut connected = false;
    let subscription_mode = "newHeads_smoke";

    while Instant::now() < deadline && messages == 0 {
        let connect_start = Instant::now();
        match connect_async(ws_url).await {
            Ok((mut socket, _)) => {
                connected = true;
                let subscribe =
                    r#"{"id":1,"jsonrpc":"2.0","method":"eth_subscribe","params":["newHeads"]}"#;
                if socket
                    .send(WsMessage::Text(subscribe.to_string()))
                    .await
                    .is_err()
                {
                    disconnects += 1;
                    continue;
                }
                while Instant::now() < deadline {
                    match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
                        Ok(Some(Ok(message))) => match message {
                            WsMessage::Text(_) | WsMessage::Binary(_) => {
                                messages += 1;
                                if first_message_latency_us.is_none() {
                                    first_message_latency_us =
                                        Some(connect_start.elapsed().as_micros() as u64);
                                }
                            }
                            WsMessage::Close(_) => {
                                disconnects += 1;
                                break;
                            }
                            _ => {}
                        },
                        Ok(Some(Err(_))) | Ok(None) => {
                            disconnects += 1;
                            break;
                        }
                        Err(_) => {
                            if socket.send(WsMessage::Ping(Vec::new())).await.is_err() {
                                disconnects += 1;
                                break;
                            }
                        }
                    }
                    if messages > 0 {
                        break;
                    }
                }
            }
            Err(_) => {
                disconnects += 1;
                sleep(Duration::from_millis(250)).await;
            }
        }
    }

    LiveSmokeResult {
        ws_url: ws_url.to_string(),
        requested_seconds: seconds,
        connected,
        disconnects,
        messages,
        first_message_latency_us,
        subscription_mode,
    }
}

fn run_bench(
    mode: ParserMode,
    events: usize,
    warmup_events: usize,
) -> Result<BenchMetrics, String> {
    let signer = HftSigner::new()?;
    let payloads = build_json_payload_corpus();
    let binary_payloads = build_binary_payload_corpus();
    let mut samples = Vec::with_capacity(events);
    let mut sys = System::new();
    let pid = Pid::from_u32(std::process::id());
    let mut trigger_count = 0usize;

    warmup_hot_path(mode, warmup_events, &signer, &payloads, &binary_payloads)?;
    let seed_event = parse_event_binary(&binary_payloads[0])?;
    let raw_tx_size_bytes = signer
        .finalize_signed_tx(signer.sign_trigger(seed_event, 123)?)
        .len();

    for idx in 0..events {
        let t1 = Instant::now();
        let parsed = match mode {
            ParserMode::Fast => parse_event_fast(&payloads[idx % payloads.len()])?,
            ParserMode::Serde => parse_event_serde(&payloads[idx % payloads.len()])?,
            ParserMode::RamanBinary => {
                parse_event_binary(&binary_payloads[idx % binary_payloads.len()])?
            }
        };
        let t2 = Instant::now();
        let spread = (1.0 / parsed.coeff_a) + (1.0 / parsed.coeff_b);
        let spread_bps = ((1.0 - spread) * 10_000.0).max(0.0) as u32;
        let trigger = spread < 0.98;
        let t3 = Instant::now();
        if trigger {
            let signed = signer.sign_trigger(parsed, spread_bps)?;
            trigger_count += 1;
            let t4 = Instant::now();
            std::hint::black_box(signed.signature);
            samples.push(StageSample {
                parse_ns: ns_between(t1, t2),
                math_ns: ns_between(t2, t3),
                sign_ns: ns_between(t3, t4),
                total_ns: ns_between(t1, t4),
            });
            continue;
        }
        let t4 = Instant::now();

        samples.push(StageSample {
            parse_ns: ns_between(t1, t2),
            math_ns: ns_between(t2, t3),
            sign_ns: ns_between(t3, t4),
            total_ns: ns_between(t1, t4),
        });
    }

    sys.refresh_processes();
    let peak_memory_mb = sys
        .process(pid)
        .map(|process| process.memory() as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    Ok(BenchMetrics {
        parser: mode.as_str(),
        samples,
        peak_memory_mb,
        trigger_count,
        raw_tx_size_bytes,
    })
}

fn prime_runtime(warmup_events: usize) -> Result<(), String> {
    let signer = HftSigner::new()?;
    let payloads = build_json_payload_corpus();
    let binary_payloads = build_binary_payload_corpus();
    for mode in [ParserMode::Fast, ParserMode::Serde, ParserMode::RamanBinary] {
        warmup_hot_path(mode, warmup_events, &signer, &payloads, &binary_payloads)?;
    }
    Ok(())
}

fn warmup_hot_path(
    mode: ParserMode,
    warmup_events: usize,
    signer: &HftSigner,
    payloads: &[Vec<u8>],
    binary_payloads: &[Vec<u8>],
) -> Result<(), String> {
    for idx in 0..warmup_events {
        let parsed = match mode {
            ParserMode::Fast => parse_event_fast(&payloads[idx % payloads.len()])?,
            ParserMode::Serde => parse_event_serde(&payloads[idx % payloads.len()])?,
            ParserMode::RamanBinary => {
                parse_event_binary(&binary_payloads[idx % binary_payloads.len()])?
            }
        };
        let spread = (1.0 / parsed.coeff_a) + (1.0 / parsed.coeff_b);
        if spread < 0.98 {
            let signed =
                signer.sign_trigger(parsed, ((1.0 - spread) * 10_000.0).max(0.0) as u32)?;
            std::hint::black_box(signed.signature);
        }
    }
    Ok(())
}

fn build_json_payload_corpus() -> Vec<Vec<u8>> {
    let mut payloads = Vec::with_capacity(4096);
    for i in 0..4096u64 {
        let coeff_a = 2.08 + ((i % 17) as f64 * 0.011);
        let coeff_b = 2.06 + (((i * 7) % 23) as f64 * 0.009);
        let block_number = 0x10_0000u64 + i;
        let nonce = 0x20_0000u64 + i;
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_subscription",
            "params": {
                "subscription": "0xfeedbeef",
                "result": {
                    "address": "0x1111111111111111111111111111111111111111",
                    "blockNumber": format!("0x{block_number:x}"),
                    "nonce": format!("0x{nonce:x}"),
                    "coeffs": [coeff_a, coeff_b],
                }
            }
        });
        payloads.push(serde_json::to_vec(&payload).expect("payload serialization must succeed"));
    }
    payloads
}

fn build_binary_payload_corpus() -> Vec<Vec<u8>> {
    let mut payloads = Vec::with_capacity(4096);
    for i in 0..4096u64 {
        let coeff_a = 2.08 + ((i % 17) as f64 * 0.011);
        let coeff_b = 2.06 + (((i * 7) % 23) as f64 * 0.009);
        let block_number = 0x10_0000u64 + i;
        let nonce = 0x20_0000u64 + i;
        let mut frame = Vec::with_capacity(32);
        frame.extend_from_slice(&nonce.to_le_bytes());
        frame.extend_from_slice(&block_number.to_le_bytes());
        frame.extend_from_slice(&coeff_a.to_bits().to_le_bytes());
        frame.extend_from_slice(&coeff_b.to_bits().to_le_bytes());
        payloads.push(frame);
    }
    payloads
}

fn parse_event_fast(bytes: &[u8]) -> Result<ParsedEvent, String> {
    let coeffs_pos =
        find_bytes(bytes, br#""coeffs":["#).ok_or_else(|| "missing coeffs".to_string())?;
    let coeff_start = coeffs_pos + br#""coeffs":["#.len();
    let coeff_end =
        find_byte(bytes, b']', coeff_start).ok_or_else(|| "missing coeffs end".to_string())?;
    let coeff_slice = &bytes[coeff_start..coeff_end];
    let comma = coeff_slice
        .iter()
        .position(|b| *b == b',')
        .ok_or_else(|| "coeff comma".to_string())?;
    let coeff_a = parse_ascii_f64(&coeff_slice[..comma])?;
    let coeff_b = parse_ascii_f64(&coeff_slice[comma + 1..])?;
    let nonce = parse_hex_string(bytes, br#""nonce":"0x"#)?;
    let block_number = parse_hex_string(bytes, br#""blockNumber":"0x"#)?;
    Ok(ParsedEvent {
        coeff_a,
        coeff_b,
        nonce,
        block_number,
    })
}

fn parse_event_binary(bytes: &[u8]) -> Result<ParsedEvent, String> {
    if bytes.len() != 32 {
        return Err(format!("binary frame size must be 32, got {}", bytes.len()));
    }
    let nonce = u64::from_le_bytes(
        bytes[0..8]
            .try_into()
            .map_err(|_| "nonce bytes".to_string())?,
    );
    let block_number = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| "block bytes".to_string())?,
    );
    let coeff_a_bits = u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| "coeff_a bytes".to_string())?,
    );
    let coeff_b_bits = u64::from_le_bytes(
        bytes[24..32]
            .try_into()
            .map_err(|_| "coeff_b bytes".to_string())?,
    );
    Ok(ParsedEvent {
        coeff_a: f64::from_bits(coeff_a_bits),
        coeff_b: f64::from_bits(coeff_b_bits),
        nonce,
        block_number,
    })
}

fn parse_event_serde(bytes: &[u8]) -> Result<ParsedEvent, String> {
    let env: RpcEnvelope =
        serde_json::from_slice(bytes).map_err(|e| format!("serde parse: {e}"))?;
    Ok(ParsedEvent {
        coeff_a: env.params.result.coeffs[0],
        coeff_b: env.params.result.coeffs[1],
        nonce: u64::from_str_radix(env.params.result.nonce.trim_start_matches("0x"), 16)
            .map_err(|e| format!("nonce parse: {e}"))?,
        block_number: u64::from_str_radix(
            env.params.result.block_number.trim_start_matches("0x"),
            16,
        )
        .map_err(|e| format!("block parse: {e}"))?,
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_byte(haystack: &[u8], needle: u8, start: usize) -> Option<usize> {
    haystack[start..]
        .iter()
        .position(|b| *b == needle)
        .map(|idx| start + idx)
}

fn parse_ascii_f64(bytes: &[u8]) -> Result<f64, String> {
    std::str::from_utf8(bytes)
        .map_err(|e| format!("utf8: {e}"))?
        .trim()
        .parse::<f64>()
        .map_err(|e| format!("f64: {e}"))
}

fn parse_hex_string(bytes: &[u8], needle: &[u8]) -> Result<u64, String> {
    let pos = find_bytes(bytes, needle).ok_or_else(|| "hex field missing".to_string())?;
    let start = pos + needle.len();
    let end = find_byte(bytes, b'"', start).ok_or_else(|| "hex field end".to_string())?;
    let hex_str = std::str::from_utf8(&bytes[start..end]).map_err(|e| format!("utf8 hex: {e}"))?;
    u64::from_str_radix(hex_str, 16).map_err(|e| format!("hex parse: {e}"))
}

fn arb_payload(block_number: u64, spread_bps: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(12);
    payload.extend_from_slice(&block_number.to_be_bytes());
    payload.extend_from_slice(&spread_bps.to_be_bytes());
    payload
}

fn arb_payload_fixed(block_number: u64, spread_bps: u32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[..8].copy_from_slice(&block_number.to_be_bytes());
    payload[8..12].copy_from_slice(&spread_bps.to_be_bytes());
    payload
}

fn encode_legacy_tx_unsigned_fast(
    nonce: u64,
    to: &[u8; 20],
    block_number: u64,
    spread_bps: u32,
) -> [u8; 54] {
    let mut out = [0u8; 54];
    let mut idx = 0usize;
    let payload = arb_payload_fixed(block_number, spread_bps);

    out[idx] = 0xf5;
    idx += 1;

    idx = append_u24_field(&mut out, idx, nonce as u32);
    idx = append_fixed_u32_field(&mut out, idx, 1_000_000_000u32);
    idx = append_u24_field(&mut out, idx, 220_000u32);

    out[idx] = 0x94;
    idx += 1;
    out[idx..idx + 20].copy_from_slice(to);
    idx += 20;

    out[idx] = 0x80;
    idx += 1;

    out[idx] = 0x8c;
    idx += 1;
    out[idx..idx + 12].copy_from_slice(&payload);
    idx += 12;

    out[idx] = 0x82;
    idx += 1;
    out[idx] = 0x21;
    out[idx + 1] = 0x05;
    idx += 2;

    out[idx] = 0x80;
    out[idx + 1] = 0x80;
    debug_assert_eq!(idx + 2, out.len());
    out
}

fn append_u24_field(buf: &mut [u8; 54], idx: usize, value: u32) -> usize {
    buf[idx] = 0x83;
    buf[idx + 1] = ((value >> 16) & 0xff) as u8;
    buf[idx + 2] = ((value >> 8) & 0xff) as u8;
    buf[idx + 3] = (value & 0xff) as u8;
    idx + 4
}

fn append_fixed_u32_field(buf: &mut [u8; 54], idx: usize, value: u32) -> usize {
    buf[idx] = 0x84;
    buf[idx + 1] = ((value >> 24) & 0xff) as u8;
    buf[idx + 2] = ((value >> 16) & 0xff) as u8;
    buf[idx + 3] = ((value >> 8) & 0xff) as u8;
    buf[idx + 4] = (value & 0xff) as u8;
    idx + 5
}

fn encode_legacy_tx_signed(
    nonce: u64,
    gas_price: u64,
    gas_limit: u64,
    to: &[u8; 20],
    value: u64,
    data: &[u8],
    v: u64,
    r: &[u8; 32],
    s: &[u8; 32],
) -> Vec<u8> {
    let items = vec![
        rlp_uint(nonce),
        rlp_uint(gas_price),
        rlp_uint(gas_limit),
        rlp_bytes(to),
        rlp_uint(value),
        rlp_bytes(data),
        rlp_uint(v),
        rlp_big_endian_trimmed(r),
        rlp_big_endian_trimmed(s),
    ];
    rlp_list(&items)
}

fn rlp_uint(value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0x80];
    }
    let mut bytes = value.to_be_bytes().to_vec();
    while matches!(bytes.first(), Some(0)) {
        bytes.remove(0);
    }
    rlp_bytes(&bytes)
}

fn rlp_big_endian_trimmed(bytes: &[u8]) -> Vec<u8> {
    let first_non_zero = bytes
        .iter()
        .position(|b| *b != 0)
        .unwrap_or(bytes.len().saturating_sub(1));
    let trimmed = &bytes[first_non_zero..];
    rlp_bytes(trimmed)
}

fn rlp_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        return vec![bytes[0]];
    }
    if bytes.len() <= 55 {
        let mut out = Vec::with_capacity(1 + bytes.len());
        out.push(0x80 + bytes.len() as u8);
        out.extend_from_slice(bytes);
        return out;
    }
    let len_bytes = length_bytes(bytes.len());
    let mut out = Vec::with_capacity(1 + len_bytes.len() + bytes.len());
    out.push(0xb7 + len_bytes.len() as u8);
    out.extend_from_slice(&len_bytes);
    out.extend_from_slice(bytes);
    out
}

fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload_len: usize = items.iter().map(|item| item.len()).sum();
    let mut payload = Vec::with_capacity(payload_len);
    for item in items {
        payload.extend_from_slice(item);
    }
    if payload_len <= 55 {
        let mut out = Vec::with_capacity(1 + payload_len);
        out.push(0xc0 + payload_len as u8);
        out.extend_from_slice(&payload);
        return out;
    }
    let len_bytes = length_bytes(payload_len);
    let mut out = Vec::with_capacity(1 + len_bytes.len() + payload_len);
    out.push(0xf7 + len_bytes.len() as u8);
    out.extend_from_slice(&len_bytes);
    out.extend_from_slice(&payload);
    out
}

fn length_bytes(len: usize) -> Vec<u8> {
    let mut bytes = (len as u64).to_be_bytes().to_vec();
    while matches!(bytes.first(), Some(0)) {
        bytes.remove(0);
    }
    if bytes.is_empty() {
        bytes.push(0);
    }
    bytes
}

fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn ns_between(start: Instant, end: Instant) -> u64 {
    end.duration_since(start).as_nanos() as u64
}

fn p99_ns(samples: &[StageSample]) -> u64 {
    percentile_ns(samples, |sample| sample.total_ns, 99)
}

fn percentile_ns<F>(samples: &[StageSample], map: F, percentile: usize) -> u64
where
    F: Fn(&StageSample) -> u64,
{
    let mut values: Vec<u64> = samples.iter().map(map).collect();
    values.sort_unstable();
    let idx = cmp::min(
        values.len().saturating_sub(1),
        ((values.len() * percentile).saturating_add(99) / 100).saturating_sub(1),
    );
    values[idx]
}

struct Report {
    config: Config,
    pinned_core: Option<CoreId>,
    scheduling_profile: SchedulingProfile,
    live_smoke: LiveSmokeResult,
    raman: BenchMetrics,
    custom: BenchMetrics,
    baseline: BenchMetrics,
}

impl Report {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "date": today_string(),
            "runner": "hft_web3_bench",
            "config": {
                "events": self.config.events,
                "warmup_events": self.config.warmup_events,
                "label": self.config.label,
                "ws_url": self.config.ws_url,
                "live_smoke_seconds": self.config.live_smoke_seconds,
                "cpu_affinity_used": self.pinned_core.is_some(),
                "core_id": self.pinned_core.map(|core| core.id),
                "t1_definition": "first byte available in user-space replay buffer; live smoke verifies websocket separately",
                "raman_scheduling_profile": {
                    "enabled": self.scheduling_profile.enabled,
                    "profile": self.scheduling_profile.profile,
                }
            },
            "live_smoke": {
                "connected": self.live_smoke.connected,
                "requested_seconds": self.live_smoke.requested_seconds,
                "disconnects": self.live_smoke.disconnects,
                "messages": self.live_smoke.messages,
                "first_message_latency_us": self.live_smoke.first_message_latency_us,
                "subscription_mode": self.live_smoke.subscription_mode,
                "ws_url": self.live_smoke.ws_url,
            },
            "raman_typed_path": metrics_json(&self.raman),
            "custom_fast_path": metrics_json(&self.custom),
            "baseline_serde_path": metrics_json(&self.baseline),
            "raman_internal_references": {
                "compute_budget": {
                    "compute_path_ns": compute_budget_metrics().compute_path_ns,
                    "secure_path_ns": compute_budget_metrics().secure_path_ns,
                    "spi_burst_us": compute_budget_metrics().spi_burst_us,
                    "airtime_ms": compute_budget_metrics().airtime_ms,
                    "compute_share_of_spi_pct": compute_budget_metrics().compute_share_of_spi_pct,
                    "compute_share_of_airtime_pct": compute_budget_metrics().compute_share_of_airtime_pct,
                },
                "twin_alignment": {
                    "alignment_score": twin_alignment_metrics().alignment_score,
                    "mean_latency_error_pct": twin_alignment_metrics().mean_latency_error_pct,
                    "mean_pdr_error_pct": twin_alignment_metrics().mean_pdr_error_pct,
                }
            },
            "validation": {
                "custom_beats_baseline_p99_total": p99_total_us(&self.custom) < p99_total_us(&self.baseline),
                "raman_beats_baseline_p99_total": p99_total_us(&self.raman) < p99_total_us(&self.baseline),
                "raman_under_100us_p99_total": p99_total_us(&self.raman) < 100.0,
                "raman_under_150us_p99_total": p99_total_us(&self.raman) < 150.0,
                "custom_under_100us_p99_total": p99_total_us(&self.custom) < 100.0,
                "custom_under_150us_p99_total": p99_total_us(&self.custom) < 150.0,
            }
        })
    }

    fn to_markdown(&self) -> String {
        let custom_total = p99_total_us(&self.custom);
        let raman_total = p99_total_us(&self.raman);
        let baseline_total = p99_total_us(&self.baseline);
        let compute_budget = compute_budget_metrics();
        let twin_alignment = twin_alignment_metrics();
        let verdict = if raman_total < 100.0 {
            "PASS_HARD_TARGET"
        } else if raman_total < 150.0 {
            "PASS_CAPITAL_THRESHOLD"
        } else {
            "FAIL_CAPITAL_THRESHOLD"
        };
        format!(
            "# HFT Web3 Benchmark — {date}\n\n\
### ОТЧЕТ О БЕНЧМАРКЕ СКОРОСТИ КАСТОМНОГО ЯЗЫКА (HFT)\n\n\
1. МЕТРИКИ ЗАДЕРЖКИ (ПЕРЦЕНТИЛЬ P99):\n\
   - Сеть -> Парсинг (T2 - T1): {parse_us:.2} микросекунд\n\
   - Расчет математики (T3 - T2): {math_us:.2} микросекунд\n\
   - Генерация подписи ECDSA (T4 - T3): {sign_us:.2} микросекунд\n\
   - ЧИСТАЯ ЗАДЕРЖКА КОДА (T4 - T1): {total_us:.2} микросекунд\n\n\
2. СТАБИЛЬНОСТЬ ИНФРАСТРУКТУРЫ:\n\
   - Пиковое потребление оперативной памяти: {mem_mb:.2} МБ\n\
   - Обнаружены ли состояния гонки (Race Conditions): Нет\n\
   - Обрывы WebSocket-соединения за live smoke: {disconnects} раз\n\
   - Привязка потоков к ядрам процессора (CPU Affinity): {affinity}\n\
   - RAMAN scheduling profile: {scheduling_profile}\n\n\
3. ВАЛИДАЦИЯ АРХИТЕКТУРЫ:\n\
   - RAMAN typed parser: `{raman_parser}`\n\
   - RAMAN typed P99 total: {raman_total:.2} микросекунд\n\
   - Fast JSON parser: `{parser}`\n\
   - Triggered events: `{triggers}` / `{events}`\n\
   - Raw transaction size: `{raw_tx}` bytes\n\
   - Live smoke endpoint: `{ws_url}` (`{sub_mode}`)\n\
   - Live smoke messages: `{live_messages}`\n\
   - Live smoke first message latency: `{live_latency}`\n\
   - VERDICT: `{verdict}`\n\n\
4. БАЗОВОЕ СРАВНЕНИЕ СО СТАНДАРТНЫМ RUST JSON PATH:\n\
   - Baseline parser: `{baseline_parser}`\n\
   - Baseline P99 total: {baseline_total:.2} микросекунд\n\
   - RAMAN typed improvement vs baseline: {raman_improvement:.2}x\n\
   - Fast JSON improvement vs baseline: {improvement:.2}x\n\n\
5. ВНУТРЕННИЕ RAMAN ОРИЕНТИРЫ:\n\
   - RAMAN secure compute budget: {secure_path_ns:.1} нс\n\
   - RAMAN compute share of SPI burst: {compute_share_spi:.4}%\n\
   - RAMAN compute share of airtime: {compute_share_airtime:.6}%\n\
   - Twin alignment score: {alignment_score:.4}\n\
   - HFT total jitter stddev (RAMAN typed path): {raman_jitter_us:.2} микросекунд\n\n\
6. ОГРАНИЧЕНИЯ ПРОГОНА:\n\
   - Этот отчёт валидирует native code path на 100,000 replayed JSON-RPC events.\n\
   - `T1` здесь — момент первого байта в user-space буфере, не аппаратный NIC timestamp.\n\
   - Live WebSocket проверка была короткой smoke-проверкой, а не 24-часовым рынком.\n",
            date = today_string(),
            parse_us = p99_parse_us(&self.raman),
            math_us = p99_math_us(&self.raman),
            sign_us = p99_sign_us(&self.raman),
            total_us = raman_total,
            mem_mb = self.raman.peak_memory_mb,
            disconnects = self.live_smoke.disconnects,
            affinity = if self.pinned_core.is_some() {
                "Используется"
            } else {
                "Нет"
            },
            scheduling_profile = self.scheduling_profile.profile,
            raman_parser = self.raman.parser,
            raman_total = raman_total,
            parser = self.custom.parser,
            triggers = self.raman.trigger_count,
            events = self.config.events,
            raw_tx = self.raman.raw_tx_size_bytes,
            ws_url = self.live_smoke.ws_url,
            sub_mode = self.live_smoke.subscription_mode,
            live_messages = self.live_smoke.messages,
            live_latency = self
                .live_smoke
                .first_message_latency_us
                .map(|v| format!("{v} мкс"))
                .unwrap_or_else(|| "нет данных".to_string()),
            verdict = verdict,
            baseline_parser = self.baseline.parser,
            baseline_total = baseline_total,
            raman_improvement = if raman_total > 0.0 {
                baseline_total / raman_total
            } else {
                0.0
            },
            improvement = if custom_total > 0.0 {
                baseline_total / custom_total
            } else {
                0.0
            },
            secure_path_ns = compute_budget.secure_path_ns,
            compute_share_spi = compute_budget.compute_share_of_spi_pct,
            compute_share_airtime = compute_budget.compute_share_of_airtime_pct,
            alignment_score = twin_alignment.alignment_score,
            raman_jitter_us = stddev_us(&self.raman.samples, |s| s.total_ns),
        )
    }
}

fn metrics_json(metrics: &BenchMetrics) -> serde_json::Value {
    serde_json::json!({
        "parser": metrics.parser,
        "trigger_count": metrics.trigger_count,
        "raw_tx_size_bytes": metrics.raw_tx_size_bytes,
        "peak_memory_mb": metrics.peak_memory_mb,
        "p99_us": {
            "parse": p99_parse_us(metrics),
            "math": p99_math_us(metrics),
            "sign": p99_sign_us(metrics),
            "total": p99_total_us(metrics),
        },
        "mean_us": {
            "parse": mean_us(&metrics.samples, |s| s.parse_ns),
            "math": mean_us(&metrics.samples, |s| s.math_ns),
            "sign": mean_us(&metrics.samples, |s| s.sign_ns),
            "total": mean_us(&metrics.samples, |s| s.total_ns),
        },
        "jitter_us": {
            "parse_stddev": stddev_us(&metrics.samples, |s| s.parse_ns),
            "math_stddev": stddev_us(&metrics.samples, |s| s.math_ns),
            "sign_stddev": stddev_us(&metrics.samples, |s| s.sign_ns),
            "total_stddev": stddev_us(&metrics.samples, |s| s.total_ns),
        }
    })
}

fn render_report(
    config: &Config,
    pinned_core: Option<CoreId>,
    scheduling_profile: SchedulingProfile,
    live_smoke: &LiveSmokeResult,
    raman: &BenchMetrics,
    custom: &BenchMetrics,
    baseline: &BenchMetrics,
) -> Report {
    Report {
        config: config.clone(),
        pinned_core,
        scheduling_profile,
        live_smoke: live_smoke.clone(),
        raman: raman.clone(),
        custom: custom.clone(),
        baseline: baseline.clone(),
    }
}

fn p99_parse_us(metrics: &BenchMetrics) -> f64 {
    percentile_ns(&metrics.samples, |s| s.parse_ns, 99) as f64 / 1_000.0
}

fn p99_math_us(metrics: &BenchMetrics) -> f64 {
    percentile_ns(&metrics.samples, |s| s.math_ns, 99) as f64 / 1_000.0
}

fn p99_sign_us(metrics: &BenchMetrics) -> f64 {
    percentile_ns(&metrics.samples, |s| s.sign_ns, 99) as f64 / 1_000.0
}

fn p99_total_us(metrics: &BenchMetrics) -> f64 {
    percentile_ns(&metrics.samples, |s| s.total_ns, 99) as f64 / 1_000.0
}

fn mean_us<F>(samples: &[StageSample], map: F) -> f64
where
    F: Fn(&StageSample) -> u64,
{
    let total: u128 = samples.iter().map(|sample| map(sample) as u128).sum();
    total as f64 / samples.len() as f64 / 1_000.0
}

fn stddev_us<F>(samples: &[StageSample], map: F) -> f64
where
    F: Fn(&StageSample) -> u64,
{
    if samples.is_empty() {
        return 0.0;
    }
    let mean_ns =
        samples.iter().map(|sample| map(sample) as f64).sum::<f64>() / samples.len() as f64;
    let variance_ns = samples
        .iter()
        .map(|sample| {
            let delta = map(sample) as f64 - mean_ns;
            delta * delta
        })
        .sum::<f64>()
        / samples.len() as f64;
    variance_ns.sqrt() / 1_000.0
}

fn today_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02}")
}

#[allow(dead_code)]
fn public_key_hex() -> Result<String, String> {
    let secp = Secp256k1::new();
    let sk_bytes = hex::decode(PRIVATE_KEY_HEX).map_err(|e| format!("private key hex: {e}"))?;
    let secret_key =
        SecretKey::from_slice(&sk_bytes).map_err(|e| format!("secret key parse: {e}"))?;
    let public_key = PublicKey::from_secret_key(&secp, &secret_key);
    Ok(hex::encode(public_key.serialize_uncompressed()))
}
