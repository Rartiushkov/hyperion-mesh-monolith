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
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cis_addr =
        std::env::var("HYPERION_CARD_WEBHOOK_ADDR").unwrap_or_else(|_| "0.0.0.0:8082".to_string());
    let intl_addr =
        std::env::var("HYPERION_INTERNATIONAL_ADDR").unwrap_or_else(|_| "0.0.0.0:8083".to_string());
    let grpc_upstream = std::env::var("HYPERION_GRPC_UPSTREAM")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let codego_webhook_secret = std::env::var("CODEGO_WEBHOOK_SECRET").unwrap_or_default();
    let operator_api_token = std::env::var("HYPERION_OPERATOR_API_TOKEN").unwrap_or_default();
    let state = AppState {
        grpc_upstream,
        codego_webhook_secret,
        operator_api_token,
    };

    let cis_app = Router::new()
        .route("/aaio/webhook", post(process_aaio_license_webhook))
        .with_state(state.clone());
    let intl_app = Router::new()
        .route("/card/authorize", post(process_card_authorization))
        .route("/codego/kyc/session", post(process_codego_kyc_session))
        .route("/shared-kyc/process", post(process_codego_kyc_session))
        .route("/codego/webhook", post(process_codego_webhook))
        .route("/tbank/payments/init", post(init_tbank_payment))
        .route("/tbank/payments/state", post(get_tbank_payment_state))
        .route("/tbank/notifications", post(process_tbank_notification))
        .route("/tbank/cards/add", post(init_tbank_card_binding))
        .route("/tbank/cards/add/state", post(get_tbank_card_binding_state))
        .route(
            "/wallet/apple/provision",
            post(process_apple_wallet_provisioning),
        )
        .route("/internet/credentials", post(generate_internet_credentials))
        .route("/cards/permanent/request", post(request_permanent_card))
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
    operator_api_token: String,
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
    provider_response: serde_json::Value,
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
    provider_response: serde_json::Value,
}

#[derive(Deserialize)]
struct InternetCredentialsRequest {
    account_id: String,
    user_id: String,
    amount_minor: u64,
    currency: String,
    ttl_seconds: Option<u64>,
}

#[derive(Serialize)]
struct InternetCredentialsReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response: serde_json::Value,
    expires_in_seconds: u64,
}

#[derive(Deserialize)]
struct PermanentCardRequest {
    account_id: String,
    user_id: String,
    cardholder_name: Option<String>,
    card_type: Option<String>,
    delivery_mode: Option<String>,
    shipping_address_json: Option<serde_json::Value>,
    provider_payload: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct PermanentCardReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response: serde_json::Value,
}

#[derive(Deserialize)]
struct TbankPaymentInitRequest {
    amount_minor: u64,
    order_id: String,
    description: Option<String>,
    customer_key: Option<String>,
    payment_method: Option<String>,
    pay_type: Option<String>,
    recurrent: Option<bool>,
    operation_initiator_type: Option<String>,
    success_url: Option<String>,
    fail_url: Option<String>,
    notification_url: Option<String>,
    language: Option<String>,
    redirect_due_date: Option<String>,
    qr_data_type: Option<String>,
    qr_bank_id: Option<String>,
    receipt: Option<serde_json::Value>,
    data: Option<serde_json::Value>,
    device: Option<String>,
    device_os: Option<String>,
    device_web_view: Option<bool>,
    provider_payload: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct TbankPaymentInitReply {
    success: bool,
    payment_id: String,
    status: String,
    payment_url: String,
    qr_data: String,
    qr_data_type: String,
    provider_response: serde_json::Value,
}

#[derive(Deserialize)]
struct TbankPaymentStateRequest {
    payment_id: String,
    ip: Option<String>,
}

#[derive(Serialize)]
struct TbankForwardReply {
    success: bool,
    status: String,
    provider_response: serde_json::Value,
}

#[derive(Deserialize)]
struct TbankAddCardRequest {
    customer_key: String,
    check_type: Option<String>,
    resident_state: Option<bool>,
    notification_url: Option<String>,
    ip: Option<String>,
    provider_payload: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct TbankNotificationAck {
    ok: bool,
    notification_type: String,
    status: String,
    payment_id: String,
    order_id: String,
}

async fn process_card_authorization(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CardAuthorizationHttpRequest>,
) -> Result<Json<CardAuthorizationHttpReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
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
    headers: HeaderMap,
    Json(payload): Json<CodegoKycSessionHttpRequest>,
) -> Result<Json<CodegoKycSessionHttpReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
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
        provider_response: summarize_json_response(&reply.provider_response_body),
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
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AppleProvisioningRequest>,
) -> Result<Json<AppleProvisioningReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
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
        provider_response: summarize_json_response(&response_body),
    }))
}

async fn generate_internet_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<InternetCredentialsRequest>,
) -> Result<Json<InternetCredentialsReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
    let ttl_seconds = capped_internet_credentials_ttl(payload.ttl_seconds);
    let body = serde_json::json!({
        "accountId": payload.account_id,
        "userId": payload.user_id,
        "amountMinor": payload.amount_minor,
        "currency": payload.currency,
        "ttlSeconds": ttl_seconds,
    });
    let (status, response_body) = forward_codego_api("/cards/internet-credentials", body).await?;
    Ok(Json(InternetCredentialsReply {
        accepted: status == 200 || status == 201,
        provider_http_status: status,
        provider_response: summarize_json_response(&response_body),
        expires_in_seconds: ttl_seconds,
    }))
}

async fn request_permanent_card(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<PermanentCardRequest>,
) -> Result<Json<PermanentCardReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
    let path =
        std::env::var("CODEGO_PERMANENT_CARD_PATH").unwrap_or_else(|_| "/cards/issue".to_string());
    let mut body = serde_json::json!({
        "accountId": payload.account_id,
        "userId": payload.user_id,
        "cardholderName": payload.cardholder_name.unwrap_or_default(),
        "cardType": payload.card_type.unwrap_or_else(|| "permanent".to_string()),
        "deliveryMode": payload.delivery_mode.unwrap_or_else(|| "virtual".to_string()),
    });

    if let Some(shipping_address_json) = payload.shipping_address_json {
        body["shippingAddress"] = shipping_address_json;
    }
    if let Some(provider_payload) = payload.provider_payload {
        body["providerPayload"] = provider_payload;
    }

    let (status, response_body) = forward_codego_api(&path, body).await?;
    Ok(Json(PermanentCardReply {
        accepted: status == 200 || status == 201,
        provider_http_status: status,
        provider_response: summarize_json_response(&response_body),
    }))
}

async fn init_tbank_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<TbankPaymentInitRequest>,
) -> Result<Json<TbankPaymentInitReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
    let mut body = serde_json::json!({
        "Amount": payload.amount_minor,
        "OrderId": payload.order_id,
    });
    if let Some(description) = payload.description {
        body["Description"] = serde_json::Value::String(description);
    }
    if let Some(customer_key) = payload.customer_key {
        body["CustomerKey"] = serde_json::Value::String(customer_key);
    }
    if let Some(success_url) = payload.success_url {
        body["SuccessURL"] = serde_json::Value::String(success_url);
    }
    if let Some(fail_url) = payload.fail_url {
        body["FailURL"] = serde_json::Value::String(fail_url);
    }
    if let Some(notification_url) = payload.notification_url {
        body["NotificationURL"] = serde_json::Value::String(notification_url);
    }
    if let Some(pay_type) = payload.pay_type {
        body["PayType"] = serde_json::Value::String(pay_type);
    }
    if let Some(recurrent) = payload.recurrent {
        body["Recurrent"] = serde_json::Value::String(if recurrent {
            "Y".to_string()
        } else {
            "N".to_string()
        });
    }
    if let Some(operation_initiator_type) = payload.operation_initiator_type {
        body["OperationInitiatorType"] = serde_json::Value::String(operation_initiator_type);
    }
    if let Some(language) = payload.language {
        body["Language"] = serde_json::Value::String(language);
    }
    if let Some(redirect_due_date) = payload.redirect_due_date {
        body["RedirectDueDate"] = serde_json::Value::String(redirect_due_date);
    }
    if let Some(receipt) = payload.receipt {
        body["Receipt"] = receipt;
    }
    if let Some(data) = payload.data {
        body["DATA"] = data;
    }
    if let Some(device) = payload.device {
        body["Device"] = serde_json::Value::String(device);
    }
    if let Some(device_os) = payload.device_os {
        body["DeviceOs"] = serde_json::Value::String(device_os);
    }
    if let Some(device_web_view) = payload.device_web_view {
        body["DeviceWebView"] = serde_json::Value::Bool(device_web_view);
    }
    if let Some(provider_payload) = payload.provider_payload {
        merge_json_object(&mut body, provider_payload);
    }

    let init_response = forward_tbank_eacq("Init", body).await?;
    let payment_id = json_string(&init_response, "PaymentId");
    let status = json_string(&init_response, "Status");
    let payment_url = json_string(&init_response, "PaymentURL");
    let mut qr_data = String::new();
    let mut qr_data_type = String::new();
    let payment_method = payload
        .payment_method
        .unwrap_or_else(|| "card".to_string())
        .to_ascii_lowercase();

    if payment_method == "sbp" {
        let payment_id_value = if payment_id.is_empty() {
            return Err((
                StatusCode::BAD_GATEWAY,
                "T-Bank Init did not return PaymentId for SBP flow".to_string(),
            ));
        } else {
            payment_id.clone()
        };
        qr_data_type = payload
            .qr_data_type
            .unwrap_or_else(|| "PAYLOAD".to_string())
            .to_ascii_uppercase();
        let mut qr_body = serde_json::json!({
            "PaymentId": payment_id_value,
            "DataType": qr_data_type,
        });
        if let Some(bank_id) = payload.qr_bank_id {
            qr_body["BankId"] = serde_json::Value::String(bank_id);
        }
        let qr_response = forward_tbank_eacq("GetQr", qr_body).await?;
        qr_data = json_string(&qr_response, "Data");
        return Ok(Json(TbankPaymentInitReply {
            success: json_bool(&init_response, "Success"),
            payment_id,
            status,
            payment_url,
            qr_data,
            qr_data_type,
            provider_response: serde_json::json!({
                "init": summarize_provider_value(&init_response),
                "qr": summarize_provider_value(&qr_response),
            }),
        }));
    }

    Ok(Json(TbankPaymentInitReply {
        success: json_bool(&init_response, "Success"),
        payment_id,
        status,
        payment_url,
        qr_data,
        qr_data_type,
        provider_response: summarize_provider_value(&init_response),
    }))
}

async fn get_tbank_payment_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<TbankPaymentStateRequest>,
) -> Result<Json<TbankForwardReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
    let mut body = serde_json::json!({
        "PaymentId": payload.payment_id,
    });
    if let Some(ip) = payload.ip {
        body["IP"] = serde_json::Value::String(ip);
    }
    let response = forward_tbank_eacq("GetState", body).await?;
    Ok(Json(TbankForwardReply {
        success: json_bool(&response, "Success"),
        status: json_string(&response, "Status"),
        provider_response: summarize_provider_value(&response),
    }))
}

async fn init_tbank_card_binding(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<TbankAddCardRequest>,
) -> Result<Json<TbankForwardReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
    let mut body = serde_json::json!({
        "CustomerKey": payload.customer_key,
    });
    if let Some(check_type) = payload.check_type {
        body["CheckType"] = serde_json::Value::String(check_type);
    }
    if let Some(resident_state) = payload.resident_state {
        body["ResidentState"] = serde_json::Value::Bool(resident_state);
    }
    if let Some(ip) = payload.ip {
        body["IP"] = serde_json::Value::String(ip);
    }
    if let Some(notification_url) = payload.notification_url {
        body["NotificationURL"] = serde_json::Value::String(notification_url);
    }
    if let Some(provider_payload) = payload.provider_payload {
        merge_json_object(&mut body, provider_payload);
    }
    let response = forward_tbank_eacq("AddCard", body).await?;
    Ok(Json(TbankForwardReply {
        success: json_bool(&response, "Success"),
        status: json_string(&response, "Status"),
        provider_response: summarize_provider_value(&response),
    }))
}

async fn process_tbank_notification(
    State(_state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<(StatusCode, String), (StatusCode, String)> {
    verify_tbank_notification_token(&payload)?;
    let _summary = TbankNotificationAck {
        ok: payload
            .get("Success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        notification_type: json_string(&payload, "NotificationType"),
        status: json_string(&payload, "Status"),
        payment_id: json_string(&payload, "PaymentId"),
        order_id: json_string(&payload, "OrderId"),
    };
    Ok((StatusCode::OK, "OK".to_string()))
}

async fn get_tbank_card_binding_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<TbankForwardReply>, (StatusCode, String)> {
    require_operator_auth(&state, &headers)?;
    let response = forward_tbank_eacq("GetAddCardState", payload).await?;
    Ok(Json(TbankForwardReply {
        success: json_bool(&response, "Success"),
        status: json_string(&response, "Status"),
        provider_response: summarize_provider_value(&response),
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

async fn forward_tbank_eacq(
    method: &str,
    mut body: serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, String)> {
    if tbank_mock_mode_enabled() {
        return Ok(mock_tbank_response(method, &body));
    }
    let client = reqwest::Client::builder().build().map_err(internal_error)?;
    let terminal_key = std::env::var("TBANK_EACQ_TERMINAL_KEY").map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "TBANK_EACQ_TERMINAL_KEY is not set".to_string(),
        )
    })?;
    let password = std::env::var("TBANK_EACQ_PASSWORD").map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "TBANK_EACQ_PASSWORD is not set".to_string(),
        )
    })?;
    let base = std::env::var("TBANK_EACQ_BASE_URL")
        .unwrap_or_else(|_| "https://securepay.tinkoff.ru/v2".to_string());

    body["TerminalKey"] = serde_json::Value::String(terminal_key);
    let token = build_tbank_token(&body, &password);
    body["Token"] = serde_json::Value::String(token);

    let response = client
        .post(format!("{}/{}", base.trim_end_matches('/'), method))
        .json(&body)
        .send()
        .await
        .map_err(internal_error)?;
    let response_body = response.text().await.map_err(internal_error)?;
    serde_json::from_str(&response_body).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("T-Bank JSON parse error: {e}; body={response_body}"),
        )
    })
}

fn capped_internet_credentials_ttl(requested_ttl_seconds: Option<u64>) -> u64 {
    let default_ttl = std::env::var("HYPERION_INTERNET_CARD_TTL_SECONDS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(120);
    requested_ttl_seconds.unwrap_or(default_ttl).clamp(30, 120)
}

fn build_tbank_token(body: &serde_json::Value, password: &str) -> String {
    let mut pairs = Vec::new();
    if let Some(map) = body.as_object() {
        for (key, value) in map {
            if key == "Token" || value.is_object() || value.is_array() || value.is_null() {
                continue;
            }
            pairs.push((key.clone(), scalar_to_string(value)));
        }
    }
    pairs.push(("Password".to_string(), password.to_string()));
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let joined = pairs
        .into_iter()
        .map(|(_, value)| value)
        .collect::<String>();
    format!("{:x}", Sha256::digest(joined.as_bytes()))
}

fn scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Bool(v) => {
            if *v {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        serde_json::Value::Number(v) => v.to_string(),
        serde_json::Value::String(v) => v.clone(),
        _ => String::new(),
    }
}

fn merge_json_object(target: &mut serde_json::Value, extra: serde_json::Value) {
    if let (Some(target_map), Some(extra_map)) = (target.as_object_mut(), extra.as_object()) {
        for (key, value) in extra_map {
            target_map.insert(key.clone(), value.clone());
        }
    }
}

fn json_string(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .map(|raw| match raw {
            serde_json::Value::String(inner) => inner.clone(),
            serde_json::Value::Number(inner) => inner.to_string(),
            serde_json::Value::Bool(inner) => inner.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

fn json_bool(value: &serde_json::Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(|raw| match raw {
            serde_json::Value::Bool(inner) => Some(*inner),
            serde_json::Value::String(inner) if inner.eq_ignore_ascii_case("true") => Some(true),
            serde_json::Value::String(inner) if inner.eq_ignore_ascii_case("false") => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

fn require_operator_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, String)> {
    if state.operator_api_token.trim().is_empty() {
        return Ok(());
    }
    let authorization = headers
        .get("Authorization")
        .or_else(|| headers.get("authorization"))
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Authorization header is missing".to_string(),
            )
        })?
        .to_str()
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Authorization header is invalid".to_string(),
            )
        })?;
    let expected = format!("Bearer {}", state.operator_api_token);
    if authorization != expected {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Operator token mismatch".to_string(),
        ));
    }
    Ok(())
}

fn summarize_json_response(raw: &str) -> serde_json::Value {
    let parsed = serde_json::from_str::<serde_json::Value>(raw).unwrap_or_else(|_| {
        serde_json::json!({
            "body_preview": truncate_utf8(raw, 256),
        })
    });
    summarize_provider_value(&parsed)
}

fn summarize_provider_value(value: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for key in [
        "Success",
        "Status",
        "Message",
        "Details",
        "PaymentId",
        "OrderId",
        "RequestKey",
        "CardId",
        "RebillId",
        "PaymentURL",
        "Data",
        "sessionId",
        "iframeUrl",
        "expiresAt",
        "id",
    ] {
        if let Some(found) = find_field_case_insensitive(value, key) {
            out.insert(key.to_string(), found.clone());
        }
    }
    serde_json::Value::Object(out)
}

fn find_field_case_insensitive<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Option<&'a serde_json::Value> {
    let object = value.as_object()?;
    object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(field))
        .map(|(_, value)| value)
}

fn truncate_utf8(raw: &str, max_len: usize) -> String {
    raw.chars().take(max_len).collect()
}

fn tbank_mock_mode_enabled() -> bool {
    std::env::var("TBANK_EACQ_MOCK_MODE")
        .ok()
        .map(|raw| {
            let normalized = raw.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on"
        })
        .unwrap_or(false)
}

fn mock_tbank_response(method: &str, body: &serde_json::Value) -> serde_json::Value {
    let payment_id = json_string(body, "PaymentId");
    let order_id = json_string(body, "OrderId");
    let customer_key = json_string(body, "CustomerKey");
    match method {
        "Init" => serde_json::json!({
            "Success": true,
            "Status": "NEW",
            "PaymentId": if payment_id.is_empty() { format!("mock-pay-{}", unix_ms_now()) } else { payment_id },
            "OrderId": order_id,
            "PaymentURL": format!("https://mock.tbank.local/pay/{}", if order_id.is_empty() { "order" } else { &order_id }),
            "Message": "Mock init created",
            "Details": "TBANK_EACQ_MOCK_MODE active"
        }),
        "GetQr" => serde_json::json!({
            "Success": true,
            "Status": "NEW",
            "PaymentId": payment_id,
            "Data": format!("https://mock.tbank.local/qr/{}", if payment_id.is_empty() { "payment" } else { &payment_id }),
            "Message": "Mock SBP QR generated"
        }),
        "GetState" => serde_json::json!({
            "Success": true,
            "Status": "CONFIRMED",
            "PaymentId": payment_id,
            "OrderId": order_id,
            "Amount": body.get("Amount").cloned().unwrap_or(serde_json::json!(0)),
            "Message": "Mock payment confirmed"
        }),
        "AddCard" => serde_json::json!({
            "Success": true,
            "Status": "NEW",
            "RequestKey": format!("mock-bind-{}", unix_ms_now()),
            "CustomerKey": customer_key,
            "PaymentURL": "https://mock.tbank.local/bind-card",
            "Message": "Mock card binding created"
        }),
        "GetAddCardState" => serde_json::json!({
            "Success": true,
            "Status": "COMPLETED",
            "RequestKey": json_string(body, "RequestKey"),
            "CardId": "mock-card-id",
            "RebillId": "mock-rebill-id",
            "Message": "Mock card binding completed"
        }),
        _ => serde_json::json!({
            "Success": false,
            "Status": "ERROR",
            "Message": format!("Unsupported mock method: {method}")
        }),
    }
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

fn verify_tbank_notification_token(
    payload: &serde_json::Value,
) -> Result<(), (StatusCode, String)> {
    if tbank_mock_mode_enabled() {
        return Ok(());
    }
    let token = payload
        .get("Token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Token is missing".to_string()))?;
    let password = std::env::var("TBANK_EACQ_PASSWORD").map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "TBANK_EACQ_PASSWORD is not set".to_string(),
        )
    })?;
    let expected = build_tbank_token(payload, &password);
    if token != expected {
        return Err((
            StatusCode::UNAUTHORIZED,
            "T-Bank notification token mismatch".to_string(),
        ));
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
