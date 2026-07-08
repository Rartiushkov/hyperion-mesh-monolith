use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tonic::Request;

pub mod hyperion_gate {
    tonic::include_proto!("hyperion.gate");
}

use hyperion_gate::hyperion_gate_client::HyperionGateClient;
use hyperion_gate::{
    AuthorizationVerdict, CardAuthorizationRequest, FiatCurrency, FrontendCommand,
    SharedKycProvider, SharedKycRequest,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = std::env::var("HYPERION_CARD_WEBHOOK_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8082".to_string());
    let grpc_upstream = std::env::var("HYPERION_GRPC_UPSTREAM")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());

    let app = Router::new()
        .route("/card/authorize", post(process_card_authorization))
        .route("/shared-kyc/process", post(process_shared_kyc))
        .with_state(AppState { grpc_upstream });

    println!("[HYPERION_WEBHOOK] HTTP listening on {bind_addr}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    grpc_upstream: String,
}

#[derive(Deserialize)]
struct CardAuthorizationHttpRequest {
    account_id: String,
    user_id: String,
    card_id: String,
    merchant_id: String,
    currency: String,
    amount_minor: u64,
    request_unix_ms: Option<u64>,
}

#[derive(Serialize)]
struct CardAuthorizationHttpReply {
    verdict: String,
    frontend_command: String,
    debited_micro_usdt: u64,
    fx_rate: f64,
    processed_in_us: u64,
    tx_count: u64,
    kyc_level: String,
}

#[derive(Deserialize)]
struct SharedKycHttpRequest {
    account_id: String,
    user_id: String,
    provider: String,
    passport_payload: serde_json::Value,
    event_unix_ms: Option<u64>,
}

#[derive(Serialize)]
struct SharedKycHttpReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response_body: String,
    kyc_level: String,
}

async fn process_card_authorization(
    State(state): State<AppState>,
    Json(payload): Json<CardAuthorizationHttpRequest>,
) -> Result<Json<CardAuthorizationHttpReply>, (axum::http::StatusCode, String)> {
    let mut client = HyperionGateClient::connect(state.grpc_upstream.clone())
        .await
        .map_err(internal_error)?;
    let reply = client
        .process_card_authorization(Request::new(CardAuthorizationRequest {
            account_id: payload.account_id.into_bytes(),
            card_id: payload.card_id.into_bytes(),
            merchant_id: payload.merchant_id.into_bytes(),
            currency: parse_currency(&payload.currency) as i32,
            amount_minor: payload.amount_minor,
            request_unix_ms: payload.request_unix_ms.unwrap_or_else(unix_ms_now),
            user_id: payload.user_id.into_bytes(),
        }))
        .await
        .map_err(internal_error)?
        .into_inner();
    let verdict = AuthorizationVerdict::try_from(reply.verdict)
        .unwrap_or(AuthorizationVerdict::Unspecified);
    let frontend_command = FrontendCommand::try_from(reply.frontend_command)
        .unwrap_or(FrontendCommand::Unspecified);

    Ok(Json(CardAuthorizationHttpReply {
        verdict: format!("{verdict:?}"),
        frontend_command: format!("{frontend_command:?}"),
        debited_micro_usdt: reply.debited_micro_usdt,
        fx_rate: reply.fx_rate,
        processed_in_us: reply.processed_in_us,
        tx_count: reply.tx_count,
        kyc_level: reply.kyc_level,
    }))
}

async fn process_shared_kyc(
    State(state): State<AppState>,
    Json(payload): Json<SharedKycHttpRequest>,
) -> Result<Json<SharedKycHttpReply>, (axum::http::StatusCode, String)> {
    let mut client = HyperionGateClient::connect(state.grpc_upstream.clone())
        .await
        .map_err(internal_error)?;
    let reply = client
        .process_shared_kyc(Request::new(SharedKycRequest {
            account_id: payload.account_id.into_bytes(),
            user_id: payload.user_id.into_bytes(),
            provider: parse_provider(&payload.provider) as i32,
            passport_payload_json: payload.passport_payload.to_string(),
            event_unix_ms: payload.event_unix_ms.unwrap_or_else(unix_ms_now),
        }))
        .await
        .map_err(internal_error)?
        .into_inner();

    Ok(Json(SharedKycHttpReply {
        accepted: reply.accepted,
        provider_http_status: reply.provider_http_status,
        provider_response_body: reply.provider_response_body,
        kyc_level: reply.kyc_level,
    }))
}

fn parse_currency(value: &str) -> FiatCurrency {
    if value.eq_ignore_ascii_case("USD") {
        FiatCurrency::Usd
    } else {
        FiatCurrency::Eur
    }
}

fn parse_provider(value: &str) -> SharedKycProvider {
    if value.eq_ignore_ascii_case("KRIPICARD") {
        SharedKycProvider::Kripicard
    } else {
        SharedKycProvider::Fyatu
    }
}

fn internal_error<E: std::fmt::Display>(error: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::BAD_GATEWAY, error.to_string())
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
