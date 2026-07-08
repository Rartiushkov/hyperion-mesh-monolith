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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = HyperionGateClient::connect("http://127.0.0.1:8080").await?;

    let account_id = b"acct-smoke-0001".to_vec();
    let user_id = b"user-smoke-0001".to_vec();

    let kyc_started = Instant::now();
    let kyc_reply = client
        .process_kyc_approval(Request::new(KycApprovalRequest {
            account_id: account_id.clone(),
            user_id: user_id.clone(),
            vendor: VerificationVendor::Sumsub as i32,
            approved: true,
            event_unix_ms: unix_ms_now() as u64,
        }))
        .await?
        .into_inner();
    let kyc_elapsed_us = kyc_started.elapsed().as_micros() as u64;

    let deposit_started = Instant::now();
    let deposit_reply = client
        .process_blockchain_deposit(Request::new(BlockchainDepositRequest {
            account_id: account_id.clone(),
            tx_hash: b"0xsmokedeposit".to_vec(),
            amount_micro_usdt: 25_000_000,
            block_unix_ms: unix_ms_now() as u64,
        }))
        .await?
        .into_inner();
    let deposit_elapsed_us = deposit_started.elapsed().as_micros() as u64;

    let auth_started = Instant::now();
    let auth_reply = client
        .process_card_authorization(Request::new(CardAuthorizationRequest {
            account_id,
            card_id: b"card-smoke-5544".to_vec(),
            merchant_id: b"merchant-idea-budva".to_vec(),
            currency: FiatCurrency::Eur as i32,
            amount_minor: 1_550,
            request_unix_ms: unix_ms_now() as u64,
            user_id,
        }))
        .await?
        .into_inner();
    let auth_elapsed_us = auth_started.elapsed().as_micros() as u64;

    let verdict = AuthorizationVerdict::try_from(auth_reply.verdict)
        .unwrap_or(AuthorizationVerdict::Unspecified);

    println!("============================================================");
    println!("   HYPERION gRPC SMOKE TEST");
    println!("============================================================");
    println!(
        "KYC: accepted={} slot={} rpc_us={}",
        kyc_reply.accepted, kyc_reply.slot, kyc_elapsed_us
    );
    println!(
        "DEPOSIT: accepted={} balance_micro_usdt={} rpc_us={}",
        deposit_reply.accepted, deposit_reply.balance_micro_usdt, deposit_elapsed_us
    );
    println!(
        "AUTH: verdict={:?} debited_micro_usdt={} fx_rate={:.6} processed_in_us={} rpc_us={} signature_len={} tx_count={} kyc_level={}",
        verdict,
        auth_reply.debited_micro_usdt,
        auth_reply.fx_rate,
        auth_reply.processed_in_us,
        auth_elapsed_us,
        auth_reply.signature.len(),
        auth_reply.tx_count,
        auth_reply.kyc_level
    );
    println!("============================================================");

    Ok(())
}

fn unix_ms_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
