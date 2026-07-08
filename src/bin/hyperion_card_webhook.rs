use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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
    let bind_addr =
        std::env::var("HYPERION_CARD_WEBHOOK_ADDR").unwrap_or_else(|_| "0.0.0.0:8082".to_string());
    let grpc_upstream = std::env::var("HYPERION_GRPC_UPSTREAM")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let codego_webhook_secret = std::env::var("CODEGO_WEBHOOK_SECRET").unwrap_or_default();

    let app = Router::new()
        .route("/card/authorize", post(process_card_authorization))
        .route("/codego/kyc/session", post(process_codego_kyc_session))
        .route("/shared-kyc/process", post(process_codego_kyc_session))
        .route("/codego/webhook", post(process_codego_webhook))
        .with_state(AppState {
            grpc_upstream,
            codego_webhook_secret,
        });

    println!("[HYPERION_WEBHOOK] HTTP listening on {bind_addr}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;
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
