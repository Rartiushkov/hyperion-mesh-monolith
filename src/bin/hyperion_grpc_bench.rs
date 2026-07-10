use serde::Serialize;
use std::cmp;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use tonic::Request;

pub mod hyperion_gate {
    tonic::include_proto!("hyperion.gate");
}

use hyperion_gate::hyperion_gate_client::HyperionGateClient;
use hyperion_gate::{
    AuthorizationVerdict, BlockchainDepositRequest, CardAuthorizationRequest, FiatCurrency,
    KycApprovalRequest, VerificationVendor,
};

const DEFAULT_EVENTS: usize = 5_000;
const DEFAULT_WARMUP: usize = 200;
const BENCH_LOG_PATH: &str = "traces/hyperion_grpc_bench_latest.json";

#[derive(Debug)]
struct CliConfig {
    events: usize,
    warmup: usize,
}

#[derive(Debug, Serialize)]
struct BenchReport {
    generated_at_unix_ms: u128,
    events: usize,
    warmup: usize,
    approved: usize,
    declined: usize,
    fx_rate_last: f64,
    client_rpc_p50_us: f64,
    client_rpc_p99_us: f64,
    client_rpc_mean_us: f64,
    server_process_p50_us: f64,
    server_process_p99_us: f64,
    server_process_mean_us: f64,
    transport_overhead_p50_us: f64,
    transport_overhead_p99_us: f64,
    transport_overhead_mean_us: f64,
    total_wall_ms: f64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_cli_config();
    let mut client = HyperionGateClient::connect("http://127.0.0.1:8080").await?;
    let account_id = b"acct-bench-0001".to_vec();

    bootstrap_account(&mut client, &account_id).await?;

    for idx in 0..config.warmup {
        let _ = issue_auth(&mut client, &account_id, idx as u64).await?;
    }

    let mut approved = 0usize;
    let mut declined = 0usize;
    let mut rpc_latencies_us = Vec::with_capacity(config.events);
    let mut server_latencies_us = Vec::with_capacity(config.events);
    let mut overhead_latencies_us = Vec::with_capacity(config.events);
    let mut fx_rate_last = 0.0;

    let wall_started = Instant::now();
    for idx in 0..config.events {
        let observed_started = Instant::now();
        let reply = issue_auth(&mut client, &account_id, (config.warmup + idx) as u64).await?;
        let rpc_us = observed_started.elapsed().as_micros() as u64;
        let server_us = reply.processed_in_us;
        let overhead_us = rpc_us.saturating_sub(server_us);
        fx_rate_last = reply.fx_rate;

        match AuthorizationVerdict::try_from(reply.verdict)
            .unwrap_or(AuthorizationVerdict::Unspecified)
        {
            AuthorizationVerdict::Approved => approved += 1,
            AuthorizationVerdict::Declined
            | AuthorizationVerdict::Unspecified
            | AuthorizationVerdict::RequiresKycUpgrade => declined += 1,
        }

        rpc_latencies_us.push(rpc_us);
        server_latencies_us.push(server_us);
        overhead_latencies_us.push(overhead_us);
    }
    let total_wall_ms = wall_started.elapsed().as_secs_f64() * 1_000.0;

    let report = BenchReport {
        generated_at_unix_ms: unix_ms_now(),
        events: config.events,
        warmup: config.warmup,
        approved,
        declined,
        fx_rate_last,
        client_rpc_p50_us: percentile_us(&rpc_latencies_us, 50),
        client_rpc_p99_us: percentile_us(&rpc_latencies_us, 99),
        client_rpc_mean_us: mean_us(&rpc_latencies_us),
        server_process_p50_us: percentile_us(&server_latencies_us, 50),
        server_process_p99_us: percentile_us(&server_latencies_us, 99),
        server_process_mean_us: mean_us(&server_latencies_us),
        transport_overhead_p50_us: percentile_us(&overhead_latencies_us, 50),
        transport_overhead_p99_us: percentile_us(&overhead_latencies_us, 99),
        transport_overhead_mean_us: mean_us(&overhead_latencies_us),
        total_wall_ms,
    };

    write_report(&report)?;
    print_report(&report);
    Ok(())
}

fn parse_cli_config() -> CliConfig {
    let mut events = DEFAULT_EVENTS;
    let mut warmup = DEFAULT_WARMUP;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--events" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<usize>() {
                        events = parsed.max(1);
                    }
                }
            }
            "--warmup" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<usize>() {
                        warmup = parsed;
                    }
                }
            }
            _ => {}
        }
    }
    CliConfig { events, warmup }
}

async fn bootstrap_account(
    client: &mut HyperionGateClient<tonic::transport::Channel>,
    account_id: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    client
        .process_kyc_approval(Request::new(KycApprovalRequest {
            account_id: account_id.to_vec(),
            user_id: b"user-bench-0001".to_vec(),
            vendor: VerificationVendor::Sumsub as i32,
            approved: true,
            event_unix_ms: unix_ms_now() as u64,
            provider_user_id: String::new(),
        }))
        .await?;
    client
        .process_blockchain_deposit(Request::new(BlockchainDepositRequest {
            account_id: account_id.to_vec(),
            tx_hash: b"0xbenchdeposit".to_vec(),
            amount_micro_usdt: 500_000_000_000,
            block_unix_ms: unix_ms_now() as u64,
        }))
        .await?;
    Ok(())
}

async fn issue_auth(
    client: &mut HyperionGateClient<tonic::transport::Channel>,
    account_id: &[u8],
    seq: u64,
) -> Result<hyperion_gate::CardAuthorizationReply, Box<dyn std::error::Error>> {
    let amount_minor = 1_000 + (seq % 9_000);
    let reply = client
        .process_card_authorization(Request::new(CardAuthorizationRequest {
            account_id: account_id.to_vec(),
            card_id: format!("card-bench-{seq:08}").into_bytes(),
            merchant_id: b"merchant-bench".to_vec(),
            currency: FiatCurrency::Eur as i32,
            amount_minor,
            request_unix_ms: unix_ms_now() as u64,
            user_id: b"user-bench-0001".to_vec(),
        }))
        .await?
        .into_inner();
    Ok(reply)
}

fn write_report(report: &BenchReport) -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(BENCH_LOG_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(report)?)?;
    Ok(())
}

fn print_report(report: &BenchReport) {
    println!("============================================================");
    println!("   HYPERION gRPC AUTHORIZATION BENCHMARK");
    println!("============================================================");
    println!(
        "[INFO] Events: {} | Warmup: {} | Approved: {} | Declined: {}",
        report.events, report.warmup, report.approved, report.declined
    );
    println!("[INFO] Report: {}", BENCH_LOG_PATH);
    println!();
    println!("---> Client Observed RPC:");
    println!(
        "     - P50 / P99 / Mean: {:.2} us / {:.2} us / {:.2} us",
        report.client_rpc_p50_us, report.client_rpc_p99_us, report.client_rpc_mean_us
    );
    println!("---> Server Processed In:");
    println!(
        "     - P50 / P99 / Mean: {:.2} us / {:.2} us / {:.2} us",
        report.server_process_p50_us, report.server_process_p99_us, report.server_process_mean_us
    );
    println!("---> Transport Overhead:");
    println!(
        "     - P50 / P99 / Mean: {:.2} us / {:.2} us / {:.2} us",
        report.transport_overhead_p50_us,
        report.transport_overhead_p99_us,
        report.transport_overhead_mean_us
    );
    println!(
        "[INFO] Total Wall Time: {:.2} ms | Last FX: {:.6}",
        report.total_wall_ms, report.fx_rate_last
    );
    println!("============================================================");
}

fn percentile_us(values: &[u64], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    let idx = cmp::min(
        values.len().saturating_sub(1),
        ((values.len() * percentile).saturating_add(99) / 100).saturating_sub(1),
    );
    values[idx] as f64
}

fn mean_us(values: &[u64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64
}

fn unix_ms_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
