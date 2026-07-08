use radnet_morphic_kernel::db::supabase::{
    basic_kyc_level, is_basic_kyc, verified_kyc_level, SupabaseClient, UserKycState,
};
use secp256k1::{Message, Secp256k1, SecretKey};
use sha3::{Digest, Keccak256};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tonic::{transport::Server, Request, Response, Status};

const ACCOUNT_SLOT_COUNT: usize = 16_384;
const INITIAL_ACCOUNT_BALANCE_MICRO_USDT: i64 = 0;
const PRIVATE_KEY_HEX: &str = "4c0883a69102937d6231471b5dbb6204fe512961708279f0f95b9f8d0d4c1f7a";
const CARD_FEE_BPS: i64 = 30;
const DEFAULT_EUR_USDT: f64 = 1.0850;
const DEFAULT_USD_USDT: f64 = 1.0000;
const FX_UPDATE_INTERVAL_MS: u64 = 100;
const FX_DRIFT_BPS: f64 = 1.0; // +/- 0.01% per tick
const SHARED_KYC_LIMIT_TX_COUNT: u64 = 100;
const KYC_LEVEL_BASIC: u8 = 0;
const KYC_LEVEL_VERIFIED: u8 = 1;

pub mod hyperion_gate {
    tonic::include_proto!("hyperion.gate");
}

use hyperion_gate::hyperion_gate_server::{HyperionGate, HyperionGateServer};
use hyperion_gate::{
    AuthorizationVerdict, BlockchainDepositReply, BlockchainDepositRequest, CardAuthorizationReply,
    CardAuthorizationRequest, FiatCurrency, FrontendCommand, KycApprovalReply,
    KycApprovalRequest, SharedKycProvider, SharedKycReply, SharedKycRequest,
    VerificationVendor,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let scheduling = apply_raman_priority_profile();
    let engine = Arc::new(HyperionEngine::new()?);
    tokio::spawn(spawn_fx_feed(engine.clone()));
    let addr = std::env::var("HYPERION_GRPC_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()?;

    println!(
        "Hyperion gRPC server listening on {addr} with scheduler profile {} (enabled={})",
        scheduling.profile, scheduling.enabled
    );
    if engine.supabase.is_some() {
        println!("[HYPERION_GRPC] Supabase state sync enabled");
    } else {
        println!("[HYPERION_GRPC] Supabase env missing; using local in-memory KYC state");
    }

    Server::builder()
        .add_service(HyperionGateServer::new(HyperionGrpcService { engine }))
        .serve(addr)
        .await?;

    Ok(())
}

#[derive(Clone)]
struct HyperionGrpcService {
    engine: Arc<HyperionEngine>,
}

#[tonic::async_trait]
impl HyperionGate for HyperionGrpcService {
    async fn process_kyc_approval(
        &self,
        request: Request<KycApprovalRequest>,
    ) -> Result<Response<KycApprovalReply>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }
        let user_key = identity_key(&req.user_id, "user_id")?;
        let slot = self
            .engine
            .ensure_account_slot(&req.account_id)
            .map_err(Status::resource_exhausted)?;
        let account = &self.engine.accounts[slot];
        account.kyc_approved.store(req.approved, Ordering::Release);
        account.last_event_unix_ms.store(
            req.event_unix_ms.max(unix_ms_now() as u64),
            Ordering::Release,
        );
        account.kyc_level.store(
            if req.approved {
                KYC_LEVEL_VERIFIED
            } else {
                KYC_LEVEL_BASIC
            },
            Ordering::Release,
        );

        if let Some(db) = &self.engine.supabase {
            db.ensure_user_record(&user_key, basic_kyc_level())
                .await
                .map_err(Status::unavailable)?;
            if req.approved {
                db.update_user_kyc_level(&user_key, verified_kyc_level())
                    .await
                    .map_err(Status::unavailable)?;
            }
        }

        let accepted = match VerificationVendor::try_from(req.vendor)
            .unwrap_or(VerificationVendor::Unspecified)
        {
            VerificationVendor::Unspecified => req.approved,
            _ => req.approved,
        };

        Ok(Response::new(KycApprovalReply {
            accepted,
            slot: slot as u64,
            processed_at_unix_ms: unix_ms_now() as u64,
        }))
    }

    async fn process_card_authorization(
        &self,
        request: Request<CardAuthorizationRequest>,
    ) -> Result<Response<CardAuthorizationReply>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }
        let user_key = identity_key(&req.user_id, "user_id")?;
        let started = Instant::now();
        let slot = self
            .engine
            .ensure_account_slot(&req.account_id)
            .map_err(Status::resource_exhausted)?;
        let account = &self.engine.accounts[slot];

        if let Some(db) = &self.engine.supabase {
            db.ensure_user_record(&user_key, basic_kyc_level())
                .await
                .map_err(Status::unavailable)?;
        }

        let current_state = self
            .engine
            .load_user_kyc_state(account, &user_key)
            .await
            .map_err(Status::unavailable)?;

        if current_state.tx_count >= SHARED_KYC_LIMIT_TX_COUNT
            && is_basic_kyc(&current_state.kyc_level)
        {
            return Ok(Response::new(CardAuthorizationReply {
                verdict: AuthorizationVerdict::RequiresKycUpgrade as i32,
                debited_micro_usdt: 0,
                signature: Vec::new(),
                signing_hash: Vec::new(),
                fx_rate: 0.0,
                processed_in_us: started.elapsed().as_micros() as u64,
                frontend_command: FrontendCommand::OpenCardProviderWidget as i32,
                kyc_level: current_state.kyc_level,
                tx_count: current_state.tx_count,
            }));
        }

        let fx_rate = match FiatCurrency::try_from(req.currency)
            .unwrap_or(FiatCurrency::Unspecified)
        {
            FiatCurrency::Eur => f64::from_bits(self.engine.eur_usdt_bits.load(Ordering::Acquire)),
            FiatCurrency::Usd => f64::from_bits(self.engine.usd_usdt_bits.load(Ordering::Acquire)),
            FiatCurrency::Unspecified => {
                return Err(Status::invalid_argument("currency must be EUR or USD"));
            }
        };

        let amount_minor = req.amount_minor as i64;
        let amount_major = amount_minor as f64 / 100.0;
        let base_micro_usdt = (amount_major * fx_rate * 1_000_000.0).round() as i64;
        let total_micro_usdt = base_micro_usdt + ((base_micro_usdt * CARD_FEE_BPS) / 10_000);
        let approved = debit_account(account, total_micro_usdt);
        account.last_event_unix_ms.store(
            req.request_unix_ms.max(unix_ms_now() as u64),
            Ordering::Release,
        );

        if !approved {
            return Ok(Response::new(CardAuthorizationReply {
                verdict: AuthorizationVerdict::Declined as i32,
                debited_micro_usdt: 0,
                signature: Vec::new(),
                signing_hash: Vec::new(),
                fx_rate,
                processed_in_us: started.elapsed().as_micros() as u64,
                frontend_command: FrontendCommand::Unspecified as i32,
                kyc_level: current_state.kyc_level,
                tx_count: current_state.tx_count,
            }));
        }

        let tx_state = match self.engine.bump_user_tx_count(account, &user_key).await {
            Ok(state) => state,
            Err(error) => {
                credit_account(account, total_micro_usdt);
                return Err(Status::unavailable(error));
            }
        };
        let (signature, hash) = self.engine.signature_pool.take();
        let processed_in_us = started.elapsed().as_micros() as u64;

        Ok(Response::new(CardAuthorizationReply {
            verdict: AuthorizationVerdict::Approved as i32,
            debited_micro_usdt: total_micro_usdt as u64,
            signature: signature.to_vec(),
            signing_hash: hash.to_vec(),
            fx_rate,
            processed_in_us,
            frontend_command: FrontendCommand::Unspecified as i32,
            kyc_level: tx_state.kyc_level,
            tx_count: tx_state.tx_count,
        }))
    }

    async fn process_blockchain_deposit(
        &self,
        request: Request<BlockchainDepositRequest>,
    ) -> Result<Response<BlockchainDepositReply>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }
        let slot = self
            .engine
            .ensure_account_slot(&req.account_id)
            .map_err(Status::resource_exhausted)?;
        let account = &self.engine.accounts[slot];
        let next_balance = account
            .balance_micro_usdt
            .fetch_add(req.amount_micro_usdt as i64, Ordering::AcqRel)
            + req.amount_micro_usdt as i64;
        account.last_event_unix_ms.store(
            req.block_unix_ms.max(unix_ms_now() as u64),
            Ordering::Release,
        );

        Ok(Response::new(BlockchainDepositReply {
            accepted: true,
            balance_micro_usdt: next_balance.max(0) as u64,
            processed_at_unix_ms: unix_ms_now() as u64,
        }))
    }

    async fn process_shared_kyc(
        &self,
        request: Request<SharedKycRequest>,
    ) -> Result<Response<SharedKycReply>, Status> {
        let req = request.into_inner();
        let user_key = identity_key(&req.user_id, "user_id")?;
        let slot = self
            .engine
            .ensure_account_slot(&req.account_id)
            .map_err(Status::resource_exhausted)?;
        let account = &self.engine.accounts[slot];
        if req.passport_payload_json.trim().is_empty() {
            return Err(Status::invalid_argument("passport_payload_json is required"));
        }

        let provider = SharedKycProvider::try_from(req.provider)
            .unwrap_or(SharedKycProvider::Unspecified);
        let provider_reply = self
            .engine
            .shared_kyc
            .forward_json(provider, &req.passport_payload_json)
            .await
            .map_err(Status::unavailable)?;

        if provider_reply.accepted {
            account.kyc_level.store(KYC_LEVEL_VERIFIED, Ordering::Release);
            account.kyc_approved.store(true, Ordering::Release);
            account.last_event_unix_ms.store(
                req.event_unix_ms.max(unix_ms_now() as u64),
                Ordering::Release,
            );
            if let Some(db) = &self.engine.supabase {
                db.update_user_kyc_level(&user_key, verified_kyc_level())
                    .await
                    .map_err(Status::unavailable)?;
            }
        }

        Ok(Response::new(SharedKycReply {
            accepted: provider_reply.accepted,
            provider_http_status: provider_reply.provider_http_status,
            provider_response_body: provider_reply.provider_response_body,
            kyc_level: if provider_reply.accepted {
                verified_kyc_level().to_string()
            } else {
                self.engine.local_kyc_level(account).to_string()
            },
            frontend_command: FrontendCommand::Unspecified as i32,
            processed_at_unix_ms: unix_ms_now() as u64,
        }))
    }
}

struct HyperionEngine {
    accounts: Vec<AccountSlot>,
    eur_usdt_bits: AtomicU64,
    usd_usdt_bits: AtomicU64,
    signature_pool: SignaturePool,
    supabase: Option<SupabaseClient>,
    shared_kyc: SharedKycTransit,
}

impl HyperionEngine {
    fn new() -> Result<Self, String> {
        let mut accounts = Vec::with_capacity(ACCOUNT_SLOT_COUNT);
        for _ in 0..ACCOUNT_SLOT_COUNT {
            accounts.push(AccountSlot::default());
        }
        let eur_usdt = fx_initial_rate("HYPERION_EUR_USDT", DEFAULT_EUR_USDT);
        let usd_usdt = fx_initial_rate("HYPERION_USD_USDT", DEFAULT_USD_USDT);
        Ok(Self {
            accounts,
            eur_usdt_bits: AtomicU64::new(eur_usdt),
            usd_usdt_bits: AtomicU64::new(usd_usdt),
            signature_pool: SignaturePool::new(ACCOUNT_SLOT_COUNT + 8_192)?,
            supabase: SupabaseClient::from_env()?,
            shared_kyc: SharedKycTransit::new()?,
        })
    }

    fn ensure_account_slot(&self, account_id: &[u8]) -> Result<usize, String> {
        let tag = account_tag(account_id);
        let mut slot = (tag as usize) % self.accounts.len();
        for _ in 0..self.accounts.len() {
            let current = self.accounts[slot].account_tag.load(Ordering::Acquire);
            if current == tag {
                return Ok(slot);
            }
            if current == 0 {
                match self.accounts[slot].account_tag.compare_exchange(
                    0,
                    tag,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.accounts[slot]
                            .kyc_approved
                            .store(true, Ordering::Release);
                        self.accounts[slot]
                            .kyc_level
                            .store(KYC_LEVEL_BASIC, Ordering::Release);
                        self.accounts[slot]
                            .tx_count
                            .store(0, Ordering::Release);
                        self.accounts[slot]
                            .balance_micro_usdt
                            .store(INITIAL_ACCOUNT_BALANCE_MICRO_USDT, Ordering::Release);
                        return Ok(slot);
                    }
                    Err(existing) if existing == tag => return Ok(slot),
                    Err(_) => {}
                }
            }
            slot = (slot + 1) % self.accounts.len();
        }
        Err("account slot table exhausted".to_string())
    }

    async fn load_user_kyc_state(
        &self,
        account: &AccountSlot,
        user_key: &str,
    ) -> Result<UserKycState, String> {
        if let Some(db) = &self.supabase {
            if let Some(state) = db.fetch_user_kyc_state(user_key).await? {
                account.tx_count.store(state.tx_count, Ordering::Release);
                account.kyc_level.store(
                    if is_basic_kyc(&state.kyc_level) {
                        KYC_LEVEL_BASIC
                    } else {
                        KYC_LEVEL_VERIFIED
                    },
                    Ordering::Release,
                );
                return Ok(state);
            }
        }
        Ok(UserKycState {
            tx_count: account.tx_count.load(Ordering::Acquire),
            kyc_level: self.local_kyc_level(account).to_string(),
        })
    }

    async fn bump_user_tx_count(
        &self,
        account: &AccountSlot,
        user_key: &str,
    ) -> Result<UserKycState, String> {
        let local_next = account.tx_count.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(db) = &self.supabase {
            match db.increment_user_tx_count(user_key).await {
                Ok(state) => {
                    account.tx_count.store(state.tx_count, Ordering::Release);
                    Ok(state)
                }
                Err(error) => {
                    account.tx_count.fetch_sub(1, Ordering::AcqRel);
                    Err(error)
                }
            }
        } else {
            Ok(UserKycState {
                tx_count: local_next,
                kyc_level: self.local_kyc_level(account).to_string(),
            })
        }
    }

    fn local_kyc_level(&self, account: &AccountSlot) -> &'static str {
        if account.kyc_level.load(Ordering::Acquire) == KYC_LEVEL_VERIFIED {
            verified_kyc_level()
        } else {
            basic_kyc_level()
        }
    }
}

fn fx_initial_rate(env_var: &str, default: f64) -> u64 {
    std::env::var(env_var)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(default)
        .to_bits()
}

async fn spawn_fx_feed(engine: Arc<HyperionEngine>) {
    let mut interval = tokio::time::interval(Duration::from_millis(FX_UPDATE_INTERVAL_MS));
    let mut rng = SimpleRng::new(0xF00D_BA11_2026_0707);
    loop {
        interval.tick().await;
        let drift = 1.0 + (rng.next_f64() * 2.0 - 1.0) * FX_DRIFT_BPS / 10_000.0;
        let eur = f64::from_bits(engine.eur_usdt_bits.load(Ordering::Relaxed)) * drift;
        engine.eur_usdt_bits.store(eur.to_bits(), Ordering::Relaxed);
        let drift = 1.0 + (rng.next_f64() * 2.0 - 1.0) * FX_DRIFT_BPS / 10_000.0;
        let usd = f64::from_bits(engine.usd_usdt_bits.load(Ordering::Relaxed)) * drift;
        engine.usd_usdt_bits.store(usd.to_bits(), Ordering::Relaxed);
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
}

#[derive(Default)]
struct AccountSlot {
    account_tag: AtomicU64,
    kyc_approved: AtomicBool,
    balance_micro_usdt: AtomicI64,
    last_event_unix_ms: AtomicU64,
    tx_count: AtomicU64,
    kyc_level: AtomicU8,
}

struct SignaturePool {
    signatures: Vec<[u8; 65]>,
    hashes: Vec<[u8; 32]>,
    cursor: AtomicUsize,
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
            let (recovery_id, compact) = signature.serialize_compact();
            let mut out = [0u8; 65];
            out[..64].copy_from_slice(&compact);
            out[64] = recovery_id.to_i32() as u8;
            signatures.push(out);
            hashes.push(hash);
        }

        Ok(Self {
            signatures,
            hashes,
            cursor: AtomicUsize::new(0),
        })
    }

    fn take(&self) -> (&[u8; 65], &[u8; 32]) {
        let idx = self.cursor.fetch_add(1, Ordering::AcqRel) % self.signatures.len();
        (&self.signatures[idx], &self.hashes[idx])
    }
}

#[derive(Clone)]
struct SharedKycTransit {
    client: reqwest::Client,
}

impl SharedKycTransit {
    fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("shared kyc http client build failed: {e}"))?;
        Ok(Self { client })
    }

    async fn forward_json(
        &self,
        provider: SharedKycProvider,
        passport_payload_json: &str,
    ) -> Result<ProviderTransitReply, String> {
        let (url_var, key_var) = match provider {
            SharedKycProvider::Fyatu => ("FYATU_KYC_URL", "FYATU_API_KEY"),
            SharedKycProvider::Kripicard => ("KRIPICARD_KYC_URL", "KRIPICARD_API_KEY"),
            SharedKycProvider::Unspecified => {
                return Err("shared kyc provider is required".to_string());
            }
        };
        let url = std::env::var(url_var).map_err(|_| format!("{url_var} is not set"))?;
        let api_key = std::env::var(key_var).map_err(|_| format!("{key_var} is not set"))?;
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .body(passport_payload_json.to_owned())
            .send()
            .await
            .map_err(|e| format!("shared kyc provider request failed: {e}"))?;
        let status = response.status().as_u16() as u32;
        let body = response.text().await.unwrap_or_default();
        Ok(ProviderTransitReply {
            accepted: (200..300).contains(&(status as u16)),
            provider_http_status: status,
            provider_response_body: body,
        })
    }
}

struct ProviderTransitReply {
    accepted: bool,
    provider_http_status: u32,
    provider_response_body: String,
}

fn debit_account(account: &AccountSlot, amount: i64) -> bool {
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

fn credit_account(account: &AccountSlot, amount: i64) {
    account
        .balance_micro_usdt
        .fetch_add(amount, Ordering::AcqRel);
}

fn account_tag(account_id: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in account_id {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    if hash == 0 {
        1
    } else {
        hash
    }
}

#[derive(Clone, Copy)]
struct SchedulingProfile {
    enabled: bool,
    profile: &'static str,
}

#[cfg(windows)]
fn apply_raman_priority_profile() -> SchedulingProfile {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
        HIGH_PRIORITY_CLASS, THREAD_PRIORITY_HIGHEST,
    };

    let mut enabled = false;
    let mut profile = "best_effort_default";
    unsafe {
        let process = GetCurrentProcess();
        let thread = GetCurrentThread();
        let process_high = SetPriorityClass(process, HIGH_PRIORITY_CLASS) != 0;
        let thread_high = SetThreadPriority(thread, THREAD_PRIORITY_HIGHEST) != 0;
        if process_high || thread_high {
            enabled = true;
            profile = "high_process + highest_thread";
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

fn identity_key(value: &[u8], field_name: &str) -> Result<String, Status> {
    if value.is_empty() {
        return Err(Status::invalid_argument(format!("{field_name} is required")));
    }
    match std::str::from_utf8(value) {
        Ok(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        _ => Ok(hex::encode(value)),
    }
}
