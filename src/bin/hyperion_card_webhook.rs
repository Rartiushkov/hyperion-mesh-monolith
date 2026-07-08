use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use radnet_morphic_kernel::blockchain::{build_chain_block_128, derive_physical_id, JitterStats};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tonic::Request;

type HmacSha256 = Hmac<Sha256>;

pub mod hyperion_gate {
    tonic::include_proto!("hyperion.gate");
}

use hyperion_gate::hyperion_gate_client::HyperionGateClient;
use hyperion_gate::{
    AuthorizationVerdict, CardAuthorizationRequest, FiatCurrency, FrontendCommand,
    KycApprovalRequest, SharedKycProvider, SharedKycRequest, VerificationVendor,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cis_addr =
        std::env::var("HYPERION_CARD_WEBHOOK_ADDR").unwrap_or_else(|_| "0.0.0.0:8082".to_string());
    let intl_addr =
        std::env::var("HYPERION_INTERNATIONAL_ADDR").unwrap_or_else(|_| "0.0.0.0:8083".to_string());
    let grpc_upstream = std::env::var("HYPERION_GRPC_UPSTREAM")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let codego_webhook_secret = std::env::var("CODEGO_WEBHOOK_SECRET").unwrap_or_default();
    let state = AppState {
        grpc_upstream,
        codego_webhook_secret,
    };

    let cis_app = Router::new()
        .route("/aaio/webhook", post(process_aaio_license_webhook))
        .with_state(state.clone());
    let intl_app = Router::new()
        .route("/card/authorize", post(process_card_authorization))
        .route("/codego/kyc/session", post(process_codego_kyc_session))
        .route("/shared-kyc/process", post(process_codego_kyc_session))
        .route("/codego/webhook", post(process_codego_webhook))
        .route(
            "/wallet/apple/provision",
            post(process_apple_wallet_provisioning),
        )
        .route("/internet/credentials", post(generate_internet_credentials))
        .with_state(state);

    println!("[HYPERION_WEBHOOK] CIS ingress listening on {cis_addr}");
    println!("[HYPERION_WEBHOOK] International ingress listening on {intl_addr}");
    let cis_listener = tokio::net::TcpListener::bind(&cis_addr).await?;
    let intl_listener = tokio::net::TcpListener::bind(&intl_addr).await?;

    let cis_server = axum::serve(cis_listener, cis_app);
    let intl_server = axum::serve(intl_listener, intl_app);
    tokio::try_join!(cis_server, intl_server)?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    grpc_upstream: String,
    codego_webhook_secret: String,
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
struct CodegoKycSessionHttpRequest {
    account_id: String,
    user_id: String,
    email: String,
    origin: String,
    return_url: String,
    locale: Option<String>,
    applicant_type: Option<String>,
    resume_session_id: Option<String>,
    passport_payload_json: Option<String>,
}

#[derive(Serialize)]
struct CodegoKycSessionHttpReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response_body: String,
    kyc_level: String,
    iframe_url: String,
    session_id: String,
    expires_at: String,
}

#[derive(Serialize)]
struct CodegoWebhookHttpReply {
    ok: bool,
    duplicate: bool,
    processed: bool,
    message: String,
}

#[derive(Deserialize)]
struct CodegoWebhookEnvelope {
    #[serde(rename = "type")]
    event_type: String,
    body: CodegoWebhookBody,
}

#[derive(Deserialize)]
struct CodegoWebhookBody {
    id: String,
    #[serde(rename = "externalUserId")]
    external_user_id: Option<String>,
    #[serde(rename = "applicationStatus")]
    application_status: Option<String>,
    #[serde(rename = "kycStatus")]
    kyc_status: Option<String>,
}

#[derive(Deserialize)]
struct AaioLicenseWebhookRequest {
    order_id: String,
    amount_rub: f64,
    address_l2: String,
    account_id: String,
    wholesale_cost_usd: Option<f64>,
}

#[derive(Serialize)]
struct AaioLicenseWebhookReply {
    accepted: bool,
    minted_block_hex: String,
    physical_id_hex: String,
    entropy_hash: u64,
    wholesale_cost_usd: f64,
}

#[derive(Deserialize)]
struct AppleProvisioningRequest {
    account_id: String,
    user_id: String,
    device_id: String,
    wallet_nonce: String,
}

#[derive(Serialize)]
struct AppleProvisioningReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response_body: String,
}

#[derive(Deserialize)]
struct InternetCredentialsRequest {
    account_id: String,
    user_id: String,
    amount_minor: u64,
    currency: String,
}

#[derive(Serialize)]
struct InternetCredentialsReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response_body: String,
    expires_in_seconds: u64,
}

async fn process_card_authorization(
    State(state): State<AppState>,
    Json(payload): Json<CardAuthorizationHttpRequest>,
) -> Result<Json<CardAuthorizationHttpReply>, (StatusCode, String)> {
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
    let verdict =
        AuthorizationVerdict::try_from(reply.verdict).unwrap_or(AuthorizationVerdict::Unspecified);
    let frontend_command =
        FrontendCommand::try_from(reply.frontend_command).unwrap_or(FrontendCommand::Unspecified);

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

async fn process_codego_kyc_session(
    State(state): State<AppState>,
    Json(payload): Json<CodegoKycSessionHttpRequest>,
) -> Result<Json<CodegoKycSessionHttpReply>, (StatusCode, String)> {
    let mut client = HyperionGateClient::connect(state.grpc_upstream.clone())
        .await
        .map_err(internal_error)?;
    let reply = client
        .process_shared_kyc(Request::new(SharedKycRequest {
            account_id: payload.account_id.into_bytes(),
            user_id: payload.user_id.into_bytes(),
            provider: SharedKycProvider::Codego as i32,
            email: payload.email,
            origin: payload.origin,
            locale: payload.locale.unwrap_or_else(|| "en".to_string()),
            return_url: payload.return_url,
            applicant_type: payload
                .applicant_type
                .unwrap_or_else(|| "individual".to_string()),
            resume_session_id: payload.resume_session_id.unwrap_or_default(),
            event_unix_ms: unix_ms_now(),
            passport_payload_json: payload.passport_payload_json.unwrap_or_default(),
        }))
        .await
        .map_err(internal_error)?
        .into_inner();

    Ok(Json(CodegoKycSessionHttpReply {
        accepted: reply.accepted,
        provider_http_status: reply.provider_http_status,
        provider_response_body: reply.provider_response_body,
        kyc_level: reply.kyc_level,
        iframe_url: reply.iframe_url,
        session_id: reply.session_id,
        expires_at: reply.expires_at,
    }))
}

async fn process_codego_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<CodegoWebhookHttpReply>, (StatusCode, String)> {
    verify_codego_signature(&state.codego_webhook_secret, &headers, &body)?;

    let envelope: CodegoWebhookEnvelope =
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    if envelope.event_type != "user.updated" {
        return Ok(Json(CodegoWebhookHttpReply {
            ok: true,
            duplicate: false,
            processed: false,
            message: format!("ignored event {}", envelope.event_type),
        }));
    }

    let external_user_id = envelope.body.external_user_id.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "externalUserId is missing".to_string(),
        )
    })?;
    let (user_id, account_id) = split_external_user_id(external_user_id).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "externalUserId is malformed".to_string(),
        )
    })?;
    let application_status = envelope.body.application_status.clone().unwrap_or_default();
    let kyc_status = envelope.body.kyc_status.clone().unwrap_or_default();

    let approved = application_status.eq_ignore_ascii_case("approved")
        || kyc_status.eq_ignore_ascii_case("approved");

    let should_apply = approved
        || application_status.eq_ignore_ascii_case("denied")
        || application_status.eq_ignore_ascii_case("needsinformation");

    if !should_apply {
        return Ok(Json(CodegoWebhookHttpReply {
            ok: true,
            duplicate: false,
            processed: false,
            message: format!("waiting for final outcome: {}", application_status),
        }));
    }

    let mut client = HyperionGateClient::connect(state.grpc_upstream.clone())
        .await
        .map_err(internal_error)?;
    client
        .process_kyc_approval(Request::new(KycApprovalRequest {
            account_id: account_id.into_bytes(),
            user_id: user_id.into_bytes(),
            vendor: VerificationVendor::Unspecified as i32,
            approved,
            event_unix_ms: unix_ms_now(),
            provider_user_id: envelope.body.id,
        }))
        .await
        .map_err(internal_error)?;

    Ok(Json(CodegoWebhookHttpReply {
        ok: true,
        duplicate: false,
        processed: true,
        message: if approved {
            "Codego KYC approved and synced".to_string()
        } else {
            format!(
                "Codego KYC final non-approved status synced: {}",
                application_status
            )
        },
    }))
}

async fn process_aaio_license_webhook(
    State(_state): State<AppState>,
    Json(payload): Json<AaioLicenseWebhookRequest>,
) -> Result<Json<AaioLicenseWebhookReply>, (StatusCode, String)> {
    let jitter = sample_cpu_jitter();
    let block = mint_radnet_block(
        &payload.address_l2,
        &format!("{}::{}", payload.account_id, payload.order_id),
        &jitter,
    )?;
    let wholesale_cost_usd = payload.wholesale_cost_usd.unwrap_or(3.0);
    Ok(Json(AaioLicenseWebhookReply {
        accepted: payload.amount_rub > 0.0,
        minted_block_hex: hex::encode(block.block),
        physical_id_hex: hex::encode(block.physical_id),
        entropy_hash: block.entropy_hash,
        wholesale_cost_usd,
    }))
}

async fn process_apple_wallet_provisioning(
    State(_state): State<AppState>,
    Json(payload): Json<AppleProvisioningRequest>,
) -> Result<Json<AppleProvisioningReply>, (StatusCode, String)> {
    let body = serde_json::json!({
        "accountId": payload.account_id,
        "userId": payload.user_id,
        "deviceId": payload.device_id,
        "walletNonce": payload.wallet_nonce,
    });
    let (status, response_body) =
        forward_codego_api("/wallets/apple/provisioning-payload", body).await?;
    Ok(Json(AppleProvisioningReply {
        accepted: status == 200 || status == 201,
        provider_http_status: status,
        provider_response_body: response_body,
    }))
}

async fn generate_internet_credentials(
    State(_state): State<AppState>,
    Json(payload): Json<InternetCredentialsRequest>,
) -> Result<Json<InternetCredentialsReply>, (StatusCode, String)> {
    let body = serde_json::json!({
        "accountId": payload.account_id,
        "userId": payload.user_id,
        "amountMinor": payload.amount_minor,
        "currency": payload.currency,
        "ttlSeconds": 60,
    });
    let (status, response_body) = forward_codego_api("/cards/internet-credentials", body).await?;
    Ok(Json(InternetCredentialsReply {
        accepted: status == 200 || status == 201,
        provider_http_status: status,
        provider_response_body: response_body,
        expires_in_seconds: 60,
    }))
}

async fn forward_codego_api(
    path: &str,
    body: serde_json::Value,
) -> Result<(u32, String), (StatusCode, String)> {
    let client = reqwest::Client::builder().build().map_err(internal_error)?;
    let base = std::env::var("CODEGO_API_BASE")
        .unwrap_or_else(|_| "https://vcc-sandbox.codegotech.com/api/v1".to_string());
    let api_key = std::env::var("CODEGO_API_KEY").map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "CODEGO_API_KEY is not set".to_string(),
        )
    })?;
    let response = client
        .post(format!("{}{}", base.trim_end_matches('/'), path))
        .header("X-Api-Key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(internal_error)?;
    let status = response.status().as_u16() as u32;
    let response_body = response.text().await.unwrap_or_default();
    Ok((status, response_body))
}

fn sample_cpu_jitter() -> JitterStats {
    let mut deltas = [0u64; 32];
    let mut previous = std::time::Instant::now();
    for item in &mut deltas {
        let now = std::time::Instant::now();
        *item = now.duration_since(previous).as_nanos() as u64;
        previous = now;
    }
    let mut min = u64::MAX;
    let mut max = 0u64;
    let mut sum = 0u128;
    for delta in deltas {
        min = min.min(delta);
        max = max.max(delta);
        sum += delta as u128;
    }
    let mean = (sum / deltas.len() as u128) as u64;
    let variance_sum: u128 = deltas
        .iter()
        .map(|delta| {
            let diff = (*delta as i128 - mean as i128).unsigned_abs() as u128;
            diff * diff
        })
        .sum();
    let std = ((variance_sum / deltas.len() as u128) as f64).sqrt() as u64;
    JitterStats {
        samples: deltas.len() as u32,
        mean_ns: mean.min(u32::MAX as u64) as u32,
        std_ns: std.min(u32::MAX as u64) as u32,
        min_ns: min.min(u32::MAX as u64) as u32,
        max_ns: max.min(u32::MAX as u64) as u32,
    }
}

struct MintedRadnetBlock {
    block: [u8; 128],
    physical_id: [u8; 16],
    entropy_hash: u64,
}

fn mint_radnet_block(
    address_l2: &str,
    account_id: &str,
    jitter: &JitterStats,
) -> Result<MintedRadnetBlock, (StatusCode, String)> {
    let seed = std::env::var("HYPERION_BANK_PLKA_KEY").unwrap_or_else(|_| "00".repeat(32));
    let bytes = hex::decode(seed).map_err(internal_error)?;
    if bytes.len() != 32 {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "HYPERION_BANK_PLKA_KEY must be 32 bytes hex".to_string(),
        ));
    }
    let mut plka_key = [0u8; 32];
    plka_key.copy_from_slice(&bytes);
    let deltas = [
        jitter.mean_ns as u64,
        jitter.std_ns as u64,
        jitter.min_ns as u64,
        jitter.max_ns as u64,
    ];
    let (physical_id, entropy_hash) =
        derive_physical_id(&plka_key, "manual", address_l2, 8082, &deltas);
    let prev_hash = Sha256::digest(account_id.as_bytes());
    let mut prev_hash32 = [0u8; 32];
    prev_hash32.copy_from_slice(&prev_hash);
    let block = build_chain_block_128(
        1,
        &physical_id,
        "Hyperion Coin License",
        entropy_hash,
        909,
        91,
        &prev_hash32,
        jitter,
    );
    Ok(MintedRadnetBlock {
        block,
        physical_id,
        entropy_hash,
    })
}

fn parse_currency(value: &str) -> FiatCurrency {
    if value.eq_ignore_ascii_case("USD") {
        FiatCurrency::Usd
    } else {
        FiatCurrency::Eur
    }
}

fn verify_codego_signature(
    webhook_secret: &str,
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<(), (StatusCode, String)> {
    if webhook_secret.trim().is_empty() {
        return Ok(());
    }
    let signature = headers
        .get("Signature")
        .or_else(|| headers.get("signature"))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Signature header is missing".to_string(),
            )
        })?
        .to_str()
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Signature header is invalid".to_string(),
            )
        })?;

    let mut mac = HmacSha256::new_from_slice(webhook_secret.as_bytes())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    mac.update(raw_body);
    let expected = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    if signature != expected {
        return Err((StatusCode::UNAUTHORIZED, "Signature mismatch".to_string()));
    }
    Ok(())
}

fn split_external_user_id(value: &str) -> Option<(String, String)> {
    let mut parts = value.splitn(2, "::");
    let user_id = parts.next()?.trim();
    let account_id = parts.next()?.trim();
    if user_id.is_empty() || account_id.is_empty() {
        None
    } else {
        Some((user_id.to_string(), account_id.to_string()))
    }
}

fn internal_error<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    (StatusCode::BAD_GATEWAY, error.to_string())
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
