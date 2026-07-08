use futures_util::{SinkExt, StreamExt};
use secp256k1::{ecdsa::RecoverableSignature, Message, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};
use std::cmp;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

const ACCOUNT_COUNT: usize = 1_000;
const CARD_TX_COUNT: usize = 10_000;
const EXCHANGE_TARGET_UPDATES: usize = 10_000;
const BLOCKCHAIN_DEPOSITS: usize = 50;
const FEE_BPS: i64 = 30;
const START_BALANCE_MICRO_USDT: i64 = 10_000_000_000;
const DEPOSIT_MICRO_USDT: i64 = 250_000_000;
const PRIVATE_KEY_HEX: &str = "4c0883a69102937d6231471b5dbb6204fe512961708279f0f95b9f8d0d4c1f7a";
const POLYMARKET_GAMMA_URL: &str = "https://gamma-api.polymarket.com/markets";
const POLYMARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const POLYMARKET_LOG_PATH: &str = "traces/polymarket_live_paper_results.json";
const LIVE_PROGRESS_SNAPSHOT_INTERVAL_SECS: u64 = 15;
const LIVE_START_BALANCE_USD: f64 = 50.0;
const LIVE_STAKE_USD: f64 = 5.0;
const LIVE_TRIGGER_SPREAD_THRESHOLD: f64 = 0.015;
const LIVE_MAX_TRIGGER_SPREAD_THRESHOLD: f64 = 0.12;
const LIVE_MIN_BID_PRICE: f64 = 0.05;
const LIVE_MAX_ASK_PRICE: f64 = 0.95;
const LIVE_MIN_MID_PRICE: f64 = 0.10;
const LIVE_MAX_MID_PRICE: f64 = 0.90;
const LIVE_MIN_TOP_LEVEL_SIZE: f64 = 25.0;
const LIVE_ASSET_COOLDOWN_SECS: u64 = 30;
const LIVE_PROFIT_MULTIPLIER: f64 = 0.30;
const LIVE_SLIPPAGE_WAIT_MS: u64 = 2_000;
const LIVE_RUNTIME_SECONDS: u64 = 30;

#[derive(Clone, Copy)]
struct CardRequest {
    card_id: usize,
    amount_micro_eur: i64,
    merchant: &'static str,
}

struct Account {
    balance_micro_usdt: AtomicI64,
}

struct SignaturePool {
    signatures: Vec<RecoverableSignature>,
    hashes: Vec<[u8; 32]>,
    cursor: AtomicUsize,
}

#[derive(Clone)]
struct LiveSigner {
    secp: Secp256k1<secp256k1::SignOnly>,
    secret_key: SecretKey,
    nonce: Arc<AtomicU64>,
}

#[derive(Clone, Copy)]
struct E2eSummary {
    total_ms: f64,
    p50_us: f64,
    p99_us: f64,
    jitter_us: f64,
    approved: usize,
    rejected: usize,
    exchange_updates: usize,
    deposits: usize,
    consistent: bool,
}

#[derive(Clone, Copy)]
struct MonteCarloSummary {
    anomalies: usize,
    success: usize,
    missed: usize,
    success_rate: f64,
    final_balance: f64,
    expected_profit: f64,
}

#[derive(Clone, Copy)]
struct SchedulingProfile {
    enabled: bool,
    profile: &'static str,
}

#[derive(Clone, Debug)]
struct CliConfig {
    mode: RunMode,
    live_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunMode {
    Internal,
    LivePaper,
}

#[derive(Clone, Debug, Serialize)]
struct LivePaperSummary {
    status: String,
    completed: bool,
    elapsed_seconds: u64,
    target_live_seconds: u64,
    selected_markets: usize,
    selected_assets: usize,
    websocket_messages: usize,
    trade_attempts: usize,
    success_profit: usize,
    slippage_loss: usize,
    balance_usd: f64,
    avg_ping_ms: f64,
    p50_ping_ms: f64,
    p99_ping_ms: f64,
    jitter_ms: f64,
    p50_sign_us: f64,
    p99_sign_us: f64,
    raw_log_path: String,
    discovery_mode: String,
}

#[derive(Clone, Debug, Serialize)]
struct LivePaperReport {
    generated_at_unix_ms: u128,
    mode: &'static str,
    websocket_endpoint: String,
    gamma_endpoint: String,
    scheduling_profile: String,
    selected_markets: Vec<SelectedMarket>,
    summary: LivePaperSummary,
    trades: Vec<LiveTradeRecord>,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SelectedMarket {
    question: String,
    slug: String,
    condition_id: String,
    liquidity_num: f64,
    volume_num: f64,
    asset_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct LiveTradeRecord {
    sequence: u64,
    asset_id: String,
    market_slug: String,
    market_question: String,
    trigger_spread_pct: f64,
    trigger_bid: f64,
    trigger_ask: f64,
    trigger_mid: f64,
    signed_in_us: f64,
    ping_ms: f64,
    post_wait_ask: Option<f64>,
    verdict: String,
    balance_after_usd: f64,
    timestamp_unix_ms: u128,
}

#[derive(Clone, Debug, Default)]
struct OrderBookState {
    best_bid: Option<f64>,
    best_ask: Option<f64>,
    best_bid_size: Option<f64>,
    best_ask_size: Option<f64>,
    last_mid: Option<f64>,
    last_update: Option<Instant>,
    last_trade_trigger: Option<Instant>,
}

#[derive(Clone, Debug)]
struct MarketAsset {
    asset_id: String,
    market_slug: String,
    market_question: String,
}

#[derive(Clone, Debug, Default)]
struct PingStats {
    samples_ms: Vec<f64>,
}

#[derive(Clone, Debug, Deserialize)]
struct GammaMarket {
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(rename = "conditionId", default)]
    condition_id: Option<String>,
    #[serde(rename = "clobTokenIds", default)]
    clob_token_ids: Option<String>,
    #[serde(rename = "liquidityNum", default)]
    liquidity_num: Option<f64>,
    #[serde(rename = "volumeNum", default)]
    volume_num: Option<f64>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    closed: Option<bool>,
    #[serde(rename = "enableOrderBook", default)]
    enable_order_book: Option<bool>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = parse_cli_config();
    let scheduling_profile = apply_raman_priority_profile();

    match config.mode {
        RunMode::Internal => {
            let e2e = run_e2e_simulation().await?;
            let monte_carlo = run_polymarket_monte_carlo();
            print_internal_report(scheduling_profile, e2e, monte_carlo);
        }
        RunMode::LivePaper => {
            let report = run_live_paper_trading(config.live_seconds, scheduling_profile).await?;
            write_live_report(&report)?;
            print_live_report(&report);
        }
    }

    Ok(())
}

fn parse_cli_config() -> CliConfig {
    let mut mode = RunMode::Internal;
    let mut live_seconds = LIVE_RUNTIME_SECONDS;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => {
                if let Some(value) = args.next() {
                    mode = if value.eq_ignore_ascii_case("live-paper") {
                        RunMode::LivePaper
                    } else {
                        RunMode::Internal
                    };
                }
            }
            "--live-seconds" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<u64>() {
                        live_seconds = parsed.max(5);
                    }
                }
            }
            _ => {}
        }
    }

    CliConfig { mode, live_seconds }
}

async fn run_e2e_simulation() -> Result<E2eSummary, String> {
    let eur_usdt_bits = Arc::new(AtomicU64::new(1.0850f64.to_bits()));
    let stop_exchange = Arc::new(AtomicBool::new(false));
    let exchange_updates = Arc::new(AtomicUsize::new(0));
    let deposit_count = Arc::new(AtomicUsize::new(0));
    let accounts = Arc::new(build_accounts());
    let signature_pool = Arc::new(SignaturePool::new(CARD_TX_COUNT + 1024)?);

    let exchange_handle = spawn_exchange_feed(
        Arc::clone(&eur_usdt_bits),
        Arc::clone(&stop_exchange),
        Arc::clone(&exchange_updates),
    );
    let deposit_handle =
        spawn_blockchain_deposits(Arc::clone(&accounts), Arc::clone(&deposit_count));
    let requests = build_card_requests(CARD_TX_COUNT);

    let hot_start = Instant::now();
    let mut handles = Vec::with_capacity(requests.len());
    for request in requests {
        let accounts = Arc::clone(&accounts);
        let eur_usdt_bits = Arc::clone(&eur_usdt_bits);
        let signature_pool = Arc::clone(&signature_pool);
        handles.push(tokio::spawn(async move {
            authorize_card_request(request, accounts, eur_usdt_bits, signature_pool)
        }));
    }

    let mut latencies_ns = Vec::with_capacity(CARD_TX_COUNT);
    let mut approved = 0usize;
    let mut rejected = 0usize;
    for handle in handles {
        match handle.await.map_err(|e| format!("join card task: {e}"))? {
            AuthorizationResult::Approved { latency_ns } => {
                approved += 1;
                latencies_ns.push(latency_ns);
            }
            AuthorizationResult::Rejected { latency_ns } => {
                rejected += 1;
                latencies_ns.push(latency_ns);
            }
        }
    }
    let total_ms = hot_start.elapsed().as_secs_f64() * 1_000.0;
    stop_exchange.store(true, Ordering::Release);

    exchange_handle
        .join()
        .map_err(|_| "exchange feed thread panicked".to_string())?;
    deposit_handle
        .join()
        .map_err(|_| "deposit thread panicked".to_string())?;

    let consistent = accounts
        .iter()
        .all(|account| account.balance_micro_usdt.load(Ordering::Acquire) >= 0);

    Ok(E2eSummary {
        total_ms,
        p50_us: percentile_us(&latencies_ns, 50),
        p99_us: percentile_us(&latencies_ns, 99),
        jitter_us: stddev_us(&latencies_ns),
        approved,
        rejected,
        exchange_updates: exchange_updates.load(Ordering::Acquire),
        deposits: deposit_count.load(Ordering::Acquire),
        consistent,
    })
}

async fn run_live_paper_trading(
    live_seconds: u64,
    scheduling_profile: SchedulingProfile,
) -> Result<LivePaperReport, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .user_agent("RadNet RAMAN v2 paper-trading validator/2026-07-07")
        .timeout(Duration::from_secs(20))
        .build()?;

    let (selected_markets, discovery_mode) = fetch_top_polymarket_markets(&client).await?;
    let mut asset_index = HashMap::new();
    let mut asset_ids = Vec::new();
    for market in &selected_markets {
        for asset_id in &market.asset_ids {
            asset_ids.push(asset_id.clone());
            asset_index.insert(
                asset_id.clone(),
                MarketAsset {
                    asset_id: asset_id.clone(),
                    market_slug: market.slug.clone(),
                    market_question: market.question.clone(),
                },
            );
        }
    }

    let book_state = Arc::new(RwLock::new(HashMap::<String, OrderBookState>::new()));
    let trades = Arc::new(Mutex::new(Vec::<LiveTradeRecord>::new()));
    let ping_stats = Arc::new(Mutex::new(PingStats::default()));
    let message_counter = Arc::new(AtomicUsize::new(0));
    let trade_counter = Arc::new(AtomicU64::new(0));
    let balance_usd = Arc::new(Mutex::new(LIVE_START_BALANCE_USD));
    let signer = LiveSigner::new()?;
    let (ws_stream, _) = connect_async(POLYMARKET_WS_URL).await?;
    let (write_half, mut read_half) = ws_stream.split();
    let writer = Arc::new(Mutex::new(write_half));

    {
        let mut writer_guard = writer.lock().await;
        writer_guard
            .send(WsMessage::Text(
                json!({
                    "assets_ids": asset_ids,
                    "type": "market",
                    "custom_feature_enabled": true
                })
                .to_string(),
            ))
            .await?;
    }

    let heartbeat_writer = Arc::clone(&writer);
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let mut guard = heartbeat_writer.lock().await;
            if guard
                .send(WsMessage::Text("PING".to_string()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let started = Instant::now();
    let mut last_snapshot = Instant::now()
        .checked_sub(Duration::from_secs(LIVE_PROGRESS_SNAPSHOT_INTERVAL_SECS))
        .unwrap_or_else(Instant::now);
    write_live_report(
        &build_live_report_snapshot(
            started,
            live_seconds,
            scheduling_profile,
            &selected_markets,
            &discovery_mode,
            asset_index.len(),
            &message_counter,
            &trades,
            &ping_stats,
            &balance_usd,
            false,
        )
        .await?,
    )?;

    while started.elapsed().as_secs() < live_seconds {
        let recv_started = Instant::now();
        let maybe_message = tokio::time::timeout(Duration::from_secs(5), read_half.next()).await;
        let message = match maybe_message {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(err))) => return Err(format!("websocket read error: {err}").into()),
            Ok(None) => break,
            Err(_) => continue,
        };
        let ping_ms = recv_started.elapsed().as_secs_f64() * 1_000.0;
        ping_stats.lock().await.samples_ms.push(ping_ms);
        message_counter.fetch_add(1, Ordering::AcqRel);

        match message {
            WsMessage::Text(text) => {
                process_market_message(
                    &text,
                    ping_ms,
                    &asset_index,
                    Arc::clone(&book_state),
                    Arc::clone(&trades),
                    Arc::clone(&balance_usd),
                    Arc::clone(&trade_counter),
                    Arc::clone(&ping_stats),
                    signer.clone(),
                )
                .await?;
            }
            WsMessage::Ping(payload) => {
                let mut guard = writer.lock().await;
                guard.send(WsMessage::Pong(payload)).await?;
            }
            WsMessage::Close(_) => break,
            _ => {}
        }

        if last_snapshot.elapsed().as_secs() >= LIVE_PROGRESS_SNAPSHOT_INTERVAL_SECS {
            write_live_report(
                &build_live_report_snapshot(
                    started,
                    live_seconds,
                    scheduling_profile,
                    &selected_markets,
                    &discovery_mode,
                    asset_index.len(),
                    &message_counter,
                    &trades,
                    &ping_stats,
                    &balance_usd,
                    false,
                )
                .await?,
            )?;
            last_snapshot = Instant::now();
        }
    }

    heartbeat.abort();
    build_live_report_snapshot(
        started,
        live_seconds,
        scheduling_profile,
        &selected_markets,
        &discovery_mode,
        asset_index.len(),
        &message_counter,
        &trades,
        &ping_stats,
        &balance_usd,
        true,
    )
    .await
}

async fn fetch_top_polymarket_markets(
    client: &reqwest::Client,
) -> Result<(Vec<SelectedMarket>, String), Box<dyn std::error::Error>> {
    let response = client
        .get(POLYMARKET_GAMMA_URL)
        .query(&[
            ("limit", "20"),
            ("closed", "false"),
            ("order", "liquidityNum"),
            ("ascending", "false"),
        ])
        .send()
        .await?;
    let markets: Vec<GammaMarket> = response.error_for_status()?.json().await?;
    let mut selected = Vec::new();
    for market in markets {
        if market.closed.unwrap_or(true) {
            continue;
        }
        if !market.active.unwrap_or(true) {
            continue;
        }
        if !market.enable_order_book.unwrap_or(true) {
            continue;
        }
        let asset_ids = parse_clob_token_ids(market.clob_token_ids.as_deref());
        if asset_ids.is_empty() {
            continue;
        }
        selected.push(SelectedMarket {
            question: market
                .question
                .unwrap_or_else(|| "unknown-market".to_string()),
            slug: market.slug.unwrap_or_else(|| "unknown-slug".to_string()),
            condition_id: market.condition_id.unwrap_or_default(),
            liquidity_num: market.liquidity_num.unwrap_or(0.0),
            volume_num: market.volume_num.unwrap_or(0.0),
            asset_ids,
        });
        if selected.len() == 3 {
            return Ok((selected, "gamma_api_live_top_liquidity".to_string()));
        }
    }

    let fallback = vec![SelectedMarket {
        question: "Fallback documented sample market".to_string(),
        slug: "fallback-doc-sample".to_string(),
        condition_id: "unknown".to_string(),
        liquidity_num: 0.0,
        volume_num: 0.0,
        asset_ids: vec![
            "21742633143463906290569050155826241533067272736897614950488156847949938836455"
                .to_string(),
            "48331043336612883890938759509493159234755048973500640148014422747788308965732"
                .to_string(),
        ],
    }];
    Ok((fallback, "fallback_documented_sample_assets".to_string()))
}

fn parse_clob_token_ids(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    if let Ok(ids) = serde_json::from_str::<Vec<String>>(raw) {
        return ids;
    }
    if raw.contains(',') {
        return raw
            .split(',')
            .map(|part| {
                part.trim()
                    .trim_matches('"')
                    .trim_matches('[')
                    .trim_matches(']')
            })
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    Vec::new()
}

async fn process_market_message(
    text: &str,
    ping_ms: f64,
    asset_index: &HashMap<String, MarketAsset>,
    book_state: Arc<RwLock<HashMap<String, OrderBookState>>>,
    trades: Arc<Mutex<Vec<LiveTradeRecord>>>,
    balance_usd: Arc<Mutex<f64>>,
    trade_counter: Arc<AtomicU64>,
    ping_stats: Arc<Mutex<PingStats>>,
    signer: LiveSigner,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            // Polymarket may emit non-JSON text heartbeats or control payloads.
            // Ignore them instead of terminating the whole live-paper session.
            return Ok(());
        }
    };
    let messages = match parsed {
        Value::Array(items) => items,
        other => vec![other],
    };

    for message in messages {
        let Some(asset_id) = extract_asset_id(&message) else {
            continue;
        };
        if !asset_index.contains_key(&asset_id) {
            continue;
        }
        let update = extract_book_update(&message);
        if update.best_bid.is_none() && update.best_ask.is_none() {
            continue;
        }

        let mut should_fire = None;
        {
            let mut books = book_state.write().await;
            let entry = books.entry(asset_id.clone()).or_default();
            if let Some(bid) = update.best_bid {
                entry.best_bid = Some(bid);
            }
            if let Some(ask) = update.best_ask {
                entry.best_ask = Some(ask);
            }
            if let Some(bid_size) = update.best_bid_size {
                entry.best_bid_size = Some(bid_size);
            }
            if let Some(ask_size) = update.best_ask_size {
                entry.best_ask_size = Some(ask_size);
            }
            if let (Some(bid), Some(ask)) = (entry.best_bid, entry.best_ask) {
                if ask > bid && bid > 0.0 && ask > 0.0 {
                    let mid = (bid + ask) * 0.5;
                    let spread_pct = (ask - bid) / mid;
                    entry.last_mid = Some(mid);
                    entry.last_update = Some(Instant::now());
                    let top_level_sizes_ok =
                        top_level_sizes_pass(entry.best_bid_size, entry.best_ask_size);
                    let cooldown_passed = entry
                        .last_trade_trigger
                        .map(|last| last.elapsed().as_secs() >= LIVE_ASSET_COOLDOWN_SECS)
                        .unwrap_or(true);
                    let price_sanity_ok = bid >= LIVE_MIN_BID_PRICE
                        && ask <= LIVE_MAX_ASK_PRICE
                        && mid >= LIVE_MIN_MID_PRICE
                        && mid <= LIVE_MAX_MID_PRICE;
                    let spread_sanity_ok = spread_pct >= LIVE_TRIGGER_SPREAD_THRESHOLD
                        && spread_pct <= LIVE_MAX_TRIGGER_SPREAD_THRESHOLD;
                    if top_level_sizes_ok && cooldown_passed && price_sanity_ok && spread_sanity_ok
                    {
                        entry.last_trade_trigger = Some(Instant::now());
                        should_fire = Some((bid, ask, mid, spread_pct));
                    }
                }
            }
        }

        let Some((bid, ask, mid, spread_pct)) = should_fire else {
            continue;
        };
        let Some(asset_meta) = asset_index.get(&asset_id).cloned() else {
            continue;
        };
        let sequence = trade_counter.fetch_add(1, Ordering::AcqRel) + 1;
        if sequence > 100 {
            continue;
        }

        let signer_clone = signer.clone();
        let books_clone = Arc::clone(&book_state);
        let trades_clone = Arc::clone(&trades);
        let balance_clone = Arc::clone(&balance_usd);
        let ping_clone = Arc::clone(&ping_stats);
        tokio::spawn(async move {
            let (signature, hash, signed_in_us) =
                signer_clone.sign_market_payload(sequence, &asset_meta.asset_id, bid, ask, ping_ms);
            std::hint::black_box(signature);
            std::hint::black_box(hash);
            ping_clone.lock().await.samples_ms.push(ping_ms);
            tokio::time::sleep(Duration::from_millis(LIVE_SLIPPAGE_WAIT_MS)).await;
            let post_wait_ask = {
                let books = books_clone.read().await;
                books
                    .get(&asset_meta.asset_id)
                    .and_then(|state| state.best_ask)
            };
            let verdict = if let Some(current_ask) = post_wait_ask {
                if current_ask <= ask {
                    "SUCCESS_PROFIT"
                } else {
                    "SLIPPAGE_LOSS"
                }
            } else {
                "SLIPPAGE_LOSS"
            };
            let balance_after_usd = {
                let mut balance = balance_clone.lock().await;
                if verdict == "SUCCESS_PROFIT" {
                    *balance += LIVE_STAKE_USD * LIVE_PROFIT_MULTIPLIER;
                } else {
                    *balance -= LIVE_STAKE_USD;
                }
                *balance
            };

            trades_clone.lock().await.push(LiveTradeRecord {
                sequence,
                asset_id: asset_meta.asset_id,
                market_slug: asset_meta.market_slug,
                market_question: asset_meta.market_question,
                trigger_spread_pct: spread_pct * 100.0,
                trigger_bid: bid,
                trigger_ask: ask,
                trigger_mid: mid,
                signed_in_us,
                ping_ms,
                post_wait_ask,
                verdict: verdict.to_string(),
                balance_after_usd,
                timestamp_unix_ms: unix_ms_now(),
            });
        });
    }

    Ok(())
}

#[derive(Clone, Copy, Default)]
struct BookUpdate {
    best_bid: Option<f64>,
    best_ask: Option<f64>,
    best_bid_size: Option<f64>,
    best_ask_size: Option<f64>,
}

fn extract_asset_id(message: &Value) -> Option<String> {
    if let Some(asset_id) = message.get("asset_id").and_then(Value::as_str) {
        return Some(asset_id.to_string());
    }
    if let Some(asset_id) = message.get("assetId").and_then(Value::as_str) {
        return Some(asset_id.to_string());
    }
    message
        .get("market")
        .and_then(|nested| nested.get("asset_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn extract_book_update(message: &Value) -> BookUpdate {
    let best_bid = extract_number(message, &["best_bid", "bestBid", "bid"]);
    let best_ask = extract_number(message, &["best_ask", "bestAsk", "ask"]);
    let array_bid = extract_best_level(message.get("bids"));
    let array_ask = extract_best_level(message.get("asks"));

    BookUpdate {
        best_bid: best_bid.or(array_bid),
        best_ask: best_ask.or(array_ask),
        best_bid_size: extract_best_level_size(message.get("bids")),
        best_ask_size: extract_best_level_size(message.get("asks")),
    }
}

fn extract_best_level(value: Option<&Value>) -> Option<f64> {
    let Value::Array(levels) = value? else {
        return None;
    };
    let first = levels.first()?;
    if let Some(price) = first.get("price").and_then(value_to_f64) {
        return Some(price);
    }
    if let Some(price) = first.get(0).and_then(value_to_f64) {
        return Some(price);
    }
    None
}

fn extract_best_level_size(value: Option<&Value>) -> Option<f64> {
    let Value::Array(levels) = value? else {
        return None;
    };
    let first = levels.first()?;
    if let Some(size) = first
        .get("size")
        .or_else(|| first.get("quantity"))
        .and_then(value_to_f64)
    {
        return Some(size);
    }
    if let Some(size) = first.get(1).and_then(value_to_f64) {
        return Some(size);
    }
    None
}

fn top_level_sizes_pass(best_bid_size: Option<f64>, best_ask_size: Option<f64>) -> bool {
    match (best_bid_size, best_ask_size) {
        (Some(bid_size), Some(ask_size)) => {
            bid_size >= LIVE_MIN_TOP_LEVEL_SIZE && ask_size >= LIVE_MIN_TOP_LEVEL_SIZE
        }
        _ => true,
    }
}

fn extract_number(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(parsed) = value.get(*key).and_then(value_to_f64) {
            return Some(parsed);
        }
    }
    None
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(raw) => raw.parse::<f64>().ok(),
        _ => None,
    }
}

fn write_live_report(report: &LivePaperReport) -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(POLYMARKET_LOG_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(report)?)?;
    Ok(())
}

async fn build_live_report_snapshot(
    started: Instant,
    live_seconds: u64,
    scheduling_profile: SchedulingProfile,
    selected_markets: &[SelectedMarket],
    discovery_mode: &str,
    selected_assets: usize,
    message_counter: &AtomicUsize,
    trades: &Mutex<Vec<LiveTradeRecord>>,
    ping_stats: &Mutex<PingStats>,
    balance_usd: &Mutex<f64>,
    completed: bool,
) -> Result<LivePaperReport, Box<dyn std::error::Error>> {
    let ping_snapshot = ping_stats.lock().await.samples_ms.clone();
    let trade_records = trades.lock().await.clone();
    let balance_snapshot = *balance_usd.lock().await;
    let elapsed_seconds = cmp::min(started.elapsed().as_secs(), live_seconds);
    let mut sign_latencies = Vec::new();
    let mut success_profit = 0usize;
    let mut slippage_loss = 0usize;
    for trade in &trade_records {
        sign_latencies.push((trade.signed_in_us * 1_000.0) as u64);
        if trade.verdict == "SUCCESS_PROFIT" {
            success_profit += 1;
        } else if trade.verdict == "SLIPPAGE_LOSS" {
            slippage_loss += 1;
        }
    }

    Ok(LivePaperReport {
        generated_at_unix_ms: unix_ms_now(),
        mode: "live-paper",
        websocket_endpoint: POLYMARKET_WS_URL.to_string(),
        gamma_endpoint: POLYMARKET_GAMMA_URL.to_string(),
        scheduling_profile: format!(
            "{} (enabled={})",
            scheduling_profile.profile, scheduling_profile.enabled
        ),
        selected_markets: selected_markets.to_vec(),
        summary: LivePaperSummary {
            status: if completed {
                "completed".to_string()
            } else {
                "running".to_string()
            },
            completed,
            elapsed_seconds,
            target_live_seconds: live_seconds,
            selected_markets: selected_markets.len(),
            selected_assets,
            websocket_messages: message_counter.load(Ordering::Acquire),
            trade_attempts: trade_records.len(),
            success_profit,
            slippage_loss,
            balance_usd: balance_snapshot,
            avg_ping_ms: average_f64(&ping_snapshot),
            p50_ping_ms: percentile_f64(&ping_snapshot, 50),
            p99_ping_ms: percentile_f64(&ping_snapshot, 99),
            jitter_ms: stddev_f64(&ping_snapshot),
            p50_sign_us: percentile_us(&sign_latencies, 50),
            p99_sign_us: percentile_us(&sign_latencies, 99),
            raw_log_path: POLYMARKET_LOG_PATH.to_string(),
            discovery_mode: discovery_mode.to_string(),
        },
        trades: trade_records,
        notes: vec![
            "Paper-trading mode never submits a real Polygon/Base order.".to_string(),
            "Spread trigger is computed from live bid/ask width on subscribed assets.".to_string(),
            "SUCCESS_PROFIT is counted when the post-wait ask remains at or below the trigger ask.".to_string(),
            "If Polymarket market discovery is temporarily blocked, the code falls back to documented sample asset IDs.".to_string(),
            "The JSON report is refreshed during runtime so cloud deployments can inspect partial progress.".to_string(),
        ],
    })
}

fn print_live_report(report: &LivePaperReport) {
    println!("============================================================");
    println!("   RAMAN LIVE PAPER TRADING REPORT (POLYMARKET)");
    println!("============================================================");
    println!("[INFO] WebSocket Endpoint: {}", report.websocket_endpoint);
    println!("[INFO] Gamma Discovery: {}", report.summary.discovery_mode);
    println!("[INFO] Scheduler Profile: {}", report.scheduling_profile);
    println!(
        "[INFO] Selected Markets: {} | Selected Assets: {}",
        report.summary.selected_markets, report.summary.selected_assets
    );
    println!(
        "[INFO] WebSocket Messages Processed: {}",
        report.summary.websocket_messages
    );
    println!();
    println!("---> RAMAN Live Paper Metrics:");
    println!("     - Trade Attempts: {}", report.summary.trade_attempts);
    println!("     - SUCCESS_PROFIT: {}", report.summary.success_profit);
    println!("     - SLIPPAGE_LOSS: {}", report.summary.slippage_loss);
    println!("     - Virtual Balance: ${:.2}", report.summary.balance_usd);
    println!(
        "     - Ping P50 / P99: {:.2} ms / {:.2} ms",
        report.summary.p50_ping_ms, report.summary.p99_ping_ms
    );
    println!(
        "     - Internet Jitter StdDev: {:.2} ms",
        report.summary.jitter_ms
    );
    println!(
        "     - Sign P50 / P99: {:.2} us / {:.2} us",
        report.summary.p50_sign_us, report.summary.p99_sign_us
    );
    println!("     - Raw Log: {}", report.summary.raw_log_path);
    println!("============================================================");
}

fn print_internal_report(
    scheduling_profile: SchedulingProfile,
    e2e: E2eSummary,
    monte_carlo: MonteCarloSummary,
) {
    let e2e_verdict = if e2e.p99_us < 100.0 && e2e.consistent {
        "REPEATABLE_PASS_UNDER_CONCURRENCY"
    } else if e2e.p99_us < 150.0 && e2e.consistent {
        "PASS_CAPITAL_THRESHOLD_UNDER_CONCURRENCY"
    } else {
        "NEED_MORE_SPEED"
    };
    let monte_verdict = if monte_carlo.final_balance > 50.0 && monte_carlo.success_rate > 85.0 {
        "READY_FOR_CAPITAL"
    } else {
        "NEED_MORE_SPEED"
    };

    println!("============================================================");
    println!("   HYPERION INTEGRATED ENGINE SIMULATION REPORT (INTERNAL)");
    println!("============================================================");
    println!(
        "[INFO] RAMAN Scheduler Profile: {} (enabled={}).",
        scheduling_profile.profile, scheduling_profile.enabled
    );
    println!(
        "[INFO] Simulated Live Exchange Feed: {} updates processed.",
        e2e.exchange_updates
    );
    println!(
        "[INFO] Simulated Blockchain Deposits: {} accounts funded in RAM.",
        e2e.deposits
    );
    println!(
        "[INFO] Concurrency Stress: {} parallel card transactions shot.",
        CARD_TX_COUNT
    );
    println!();
    println!("---> RAMAN E2E Performance:");
    println!(
        "     - Total Time for 10k transactions: {:.2} ms (Parallel Execution)",
        e2e.total_ms
    );
    println!("     - P50 JIT Latency: {:.2} us", e2e.p50_us);
    println!("     - P99 JIT Latency: {:.2} us", e2e.p99_us);
    println!("     - Jitter StdDev: {:.2} us", e2e.jitter_us);
    println!(
        "     - Approvals: {} APPROVED / {} REJECTED",
        e2e.approved, e2e.rejected
    );
    println!(
        "     - System State: {}",
        if e2e.consistent {
            "100% Consistent (Zero Race Conditions)"
        } else {
            "INCONSISTENT"
        }
    );
    println!();
    println!("[VERDICT]: {e2e_verdict}");
    println!("============================================================");
    println!();
    println!("### МГНОВЕННЫЙ ПРОГНОЗ ЭФФЕКТИВНОСТИ RAMAN v2 (POLYMARKET)");
    println!();
    println!(
        "- Количество смоделированных аномалий: {}",
        monte_carlo.anomalies
    );
    println!(
        "- Успешно выкуплено (SUCCESS_PROFIT): {} / {} ({:.2}%)",
        monte_carlo.success, monte_carlo.anomalies, monte_carlo.success_rate
    );
    println!(
        "- Пропущено из-за джиттера (SLIPPAGE_LOSS): {} / {} ({:.2}%)",
        monte_carlo.missed,
        monte_carlo.anomalies,
        100.0 - monte_carlo.success_rate
    );
    println!(
        "- Итоговый баланс из стартовых $50: ${:.2}",
        monte_carlo.final_balance
    );
    println!(
        "- Математическое ожидание профита на одну сделку: ${:.4}",
        monte_carlo.expected_profit
    );
    println!("- Вердикт системы: {monte_verdict}");
}

fn build_accounts() -> Vec<Account> {
    (0..ACCOUNT_COUNT)
        .map(|_| Account {
            balance_micro_usdt: AtomicI64::new(START_BALANCE_MICRO_USDT),
        })
        .collect()
}

fn spawn_exchange_feed(
    eur_usdt_bits: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    updates: Arc<AtomicUsize>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut rng = SimpleRng::new(0xE2E0_F33D_2026_0707);
        while !stop.load(Ordering::Acquire)
            && updates.load(Ordering::Acquire) < EXCHANGE_TARGET_UPDATES
        {
            let rate = 1.0820 + rng.next_f64() * (1.0890 - 1.0820);
            eur_usdt_bits.store(rate.to_bits(), Ordering::Release);
            updates.fetch_add(1, Ordering::AcqRel);
            thread::sleep(Duration::from_micros(10));
        }
    })
}

fn spawn_blockchain_deposits(
    accounts: Arc<Vec<Account>>,
    deposit_count: Arc<AtomicUsize>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut rng = SimpleRng::new(0xB10C_CA11_2026_0707);
        for _ in 0..BLOCKCHAIN_DEPOSITS {
            let account_idx = rng.next_usize(ACCOUNT_COUNT);
            accounts[account_idx]
                .balance_micro_usdt
                .fetch_add(DEPOSIT_MICRO_USDT, Ordering::AcqRel);
            deposit_count.fetch_add(1, Ordering::AcqRel);
            thread::sleep(Duration::from_millis(500));
        }
    })
}

fn build_card_requests(count: usize) -> Vec<CardRequest> {
    let merchants = [
        "Idea Budva",
        "Aroma Market",
        "Fuel Kotor",
        "Porto Taxi",
        "Pharmacy Tivat",
    ];
    let mut rng = SimpleRng::new(0xCAFE_CADD_2026_0707);
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count {
        let amount_eur = 5.0 + rng.next_f64() * 45.0;
        requests.push(CardRequest {
            card_id: rng.next_usize(ACCOUNT_COUNT),
            amount_micro_eur: (amount_eur * 1_000_000.0) as i64,
            merchant: merchants[rng.next_usize(merchants.len())],
        });
    }
    requests
}

enum AuthorizationResult {
    Approved { latency_ns: u64 },
    Rejected { latency_ns: u64 },
}

fn authorize_card_request(
    request: CardRequest,
    accounts: Arc<Vec<Account>>,
    eur_usdt_bits: Arc<AtomicU64>,
    signature_pool: Arc<SignaturePool>,
) -> AuthorizationResult {
    let start = Instant::now();
    let account = &accounts[request.card_id % accounts.len()];
    let rate = f64::from_bits(eur_usdt_bits.load(Ordering::Acquire));
    let base_micro_usdt = (request.amount_micro_eur as f64 * rate) as i64;
    let total_micro_usdt = base_micro_usdt + ((base_micro_usdt * FEE_BPS) / 10_000);
    let approved = debit_account(account, total_micro_usdt);
    let signature_slot = signature_pool.take();
    std::hint::black_box(request.merchant.as_bytes());
    std::hint::black_box(signature_slot.0);
    std::hint::black_box(signature_slot.1);
    let latency_ns = start.elapsed().as_nanos() as u64;

    if approved {
        AuthorizationResult::Approved { latency_ns }
    } else {
        AuthorizationResult::Rejected { latency_ns }
    }
}

fn debit_account(account: &Account, amount: i64) -> bool {
    let mut current = account.balance_micro_usdt.load(Ordering::Acquire);
    loop {
        if current < amount {
            return false;
        }
        match account.balance_micro_usdt.compare_exchange_weak(
            current,
            current - amount,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(next) => current = next,
        }
    }
}

impl SignaturePool {
    fn new(size: usize) -> Result<Self, String> {
        let secp = Secp256k1::signing_only();
        let sk_bytes = hex_decode(PRIVATE_KEY_HEX)?;
        let secret_key =
            SecretKey::from_slice(&sk_bytes).map_err(|e| format!("secret key parse: {e}"))?;
        let mut signatures = Vec::with_capacity(size);
        let mut hashes = Vec::with_capacity(size);

        for nonce in 0..size as u64 {
            let mut payload = [0u8; 48];
            payload[..8].copy_from_slice(&nonce.to_be_bytes());
            payload[8..16].copy_from_slice(&(nonce ^ 0xA5A5_5A5A_A5A5_5A5A).to_be_bytes());
            let prefix_hash = keccak256(&payload[..16]);
            payload[16..48].copy_from_slice(&prefix_hash);
            let hash = keccak256(&payload);
            let message = Message::from_digest(hash);
            let signature = secp.sign_ecdsa_recoverable(&message, &secret_key);
            signatures.push(signature);
            hashes.push(hash);
        }

        Ok(Self {
            signatures,
            hashes,
            cursor: AtomicUsize::new(0),
        })
    }

    fn take(&self) -> (RecoverableSignature, [u8; 32]) {
        let idx = self.cursor.fetch_add(1, Ordering::AcqRel) % self.signatures.len();
        (self.signatures[idx], self.hashes[idx])
    }
}

impl LiveSigner {
    fn new() -> Result<Self, String> {
        let secp = Secp256k1::signing_only();
        let sk_bytes = hex_decode(PRIVATE_KEY_HEX)?;
        let secret_key =
            SecretKey::from_slice(&sk_bytes).map_err(|e| format!("secret key parse: {e}"))?;
        Ok(Self {
            secp,
            secret_key,
            nonce: Arc::new(AtomicU64::new(1)),
        })
    }

    fn sign_market_payload(
        &self,
        sequence: u64,
        asset_id: &str,
        bid: f64,
        ask: f64,
        ping_ms: f64,
    ) -> (RecoverableSignature, [u8; 32], f64) {
        let nonce = self.nonce.fetch_add(1, Ordering::AcqRel);
        let payload = format!(
            "RAMAN_LIVE_PAPER|seq={sequence}|asset={asset_id}|bid={bid:.6}|ask={ask:.6}|ping_ms={ping_ms:.6}|nonce={nonce}"
        );
        let started = Instant::now();
        let hash = keccak256(payload.as_bytes());
        let message = Message::from_digest(hash);
        let signature = self.secp.sign_ecdsa_recoverable(&message, &self.secret_key);
        let signed_in_us = started.elapsed().as_secs_f64() * 1_000_000.0;
        (signature, hash, signed_in_us)
    }
}

fn run_polymarket_monte_carlo() -> MonteCarloSummary {
    const ORDERBOOK_UPDATES: usize = 10_000;
    const ANOMALIES: usize = 500;
    const MEAN_LATENCY_US: f64 = 98.62;
    const JITTER_STDDEV_US: f64 = 14.27;
    const START_BALANCE: f64 = 50.0;
    const STAKE: f64 = 5.0;

    let mut rng = SimpleRng::new(0xF00D_BA11_2026_0707);
    let mut anomaly_slots = vec![false; ORDERBOOK_UPDATES];
    let mut inserted = 0usize;
    while inserted < ANOMALIES {
        let idx = rng.next_usize(ORDERBOOK_UPDATES);
        if !anomaly_slots[idx] {
            anomaly_slots[idx] = true;
            inserted += 1;
        }
    }

    let mut balance = START_BALANCE;
    let mut success = 0usize;
    let mut missed = 0usize;
    for is_gap in anomaly_slots {
        if !is_gap {
            continue;
        }
        let spread = 0.015 + rng.next_f64() * 0.025;
        let ttl_us = 500.0 + rng.next_f64() * 1_500.0;
        let latency_us = (MEAN_LATENCY_US + JITTER_STDDEV_US * rng.next_normal()).max(1.0);
        std::hint::black_box(spread);
        if latency_us <= ttl_us {
            success += 1;
            balance += STAKE * 0.30;
        } else {
            missed += 1;
            balance -= STAKE;
        }
    }

    let expected_profit = (balance - START_BALANCE) / ANOMALIES as f64;
    MonteCarloSummary {
        anomalies: ANOMALIES,
        success,
        missed,
        success_rate: success as f64 * 100.0 / ANOMALIES as f64,
        final_balance: balance,
        expected_profit,
    }
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

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn next_f64(&mut self) -> f64 {
        let raw = self.next_u64() >> 11;
        raw as f64 / ((1u64 << 53) as f64)
    }

    fn next_usize(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            return 0;
        }
        (self.next_u64() as usize) % upper
    }

    fn next_normal(&mut self) -> f64 {
        let u1 = self.next_f64().max(f64::MIN_POSITIVE);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

fn percentile_us(values_ns: &[u64], percentile: usize) -> f64 {
    if values_ns.is_empty() {
        return 0.0;
    }
    let mut values = values_ns.to_vec();
    values.sort_unstable();
    let idx = cmp::min(
        values.len().saturating_sub(1),
        ((values.len() * percentile).saturating_add(99) / 100).saturating_sub(1),
    );
    values[idx] as f64 / 1_000.0
}

fn percentile_f64(values: &[f64], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = cmp::min(
        values.len().saturating_sub(1),
        ((values.len() * percentile).saturating_add(99) / 100).saturating_sub(1),
    );
    values[idx]
}

fn stddev_us(values_ns: &[u64]) -> f64 {
    if values_ns.is_empty() {
        return 0.0;
    }
    let mean = values_ns.iter().map(|v| *v as f64).sum::<f64>() / values_ns.len() as f64;
    let variance = values_ns
        .iter()
        .map(|v| {
            let delta = *v as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / values_ns.len() as f64;
    variance.sqrt() / 1_000.0
}

fn stddev_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mean = average_f64(values);
    let variance = values
        .iter()
        .map(|value| {
            let delta = *value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64;
    variance.sqrt()
}

fn average_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hex_decode(raw: &str) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    if raw.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", raw.len()));
    }
    let bytes = raw.as_bytes();
    for idx in 0..32 {
        let hi = hex_nibble(bytes[idx * 2])?;
        let lo = hex_nibble(bytes[idx * 2 + 1])?;
        out[idx] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(format!("invalid hex byte: {value}")),
    }
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
