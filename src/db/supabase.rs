use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

const BASIC_KYC_LEVEL: &str = "BASIC";
const VERIFIED_KYC_LEVEL: &str = "VERIFIED";

#[derive(Clone)]
pub struct SupabaseClient {
    http: reqwest::Client,
    rest_url: String,
    service_role_key: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UserKycState {
    pub tx_count: u64,
    pub kyc_level: String,
}

#[derive(Clone, Debug)]
pub struct TransactionAuditRecord<'a> {
    pub address_l2: &'a str,
    pub account_id: &'a str,
    pub transaction_kind: &'a str,
    pub status: &'a str,
    pub amount_minor: u64,
    pub currency: &'a str,
    pub provider: &'a str,
    pub reference_id: &'a str,
    pub metadata_json: &'a str,
}

#[derive(Serialize)]
struct UserUpsertPayload<'a> {
    address_l2: &'a str,
    tx_count: i32,
    kyc_level: &'a str,
}

#[derive(Serialize)]
struct UserStatePatch<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kyc_level: Option<&'a str>,
}

#[derive(Serialize)]
struct TransactionInsertPayload<'a> {
    address_l2: &'a str,
    account_id: &'a str,
    transaction_kind: &'a str,
    status: &'a str,
    amount_minor: i64,
    currency: &'a str,
    provider: &'a str,
    reference_id: &'a str,
    metadata: serde_json::Value,
}

impl SupabaseClient {
    pub fn from_env() -> Result<Option<Self>, String> {
        let rest_base = std::env::var("SUPABASE_URL").unwrap_or_default();
        let service_role_key = std::env::var("SUPABASE_SERVICE_ROLE_KEY").unwrap_or_default();
        if rest_base.is_empty() || service_role_key.is_empty() {
            return Ok(None);
        }
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("supabase http client build failed: {e}"))?;
        Ok(Some(Self {
            http,
            rest_url: format!("{}/rest/v1", rest_base.trim_end_matches('/')),
            service_role_key,
        }))
    }

    pub async fn ensure_user_record(
        &self,
        address_l2: &str,
        initial_kyc_level: &str,
    ) -> Result<(), String> {
        let mut url = reqwest::Url::parse(&format!("{}/users", self.rest_url))
            .map_err(|e| format!("supabase users url parse failed: {e}"))?;
        url.query_pairs_mut()
            .append_pair("on_conflict", "address_l2");
        let payload = [UserUpsertPayload {
            address_l2,
            tx_count: 0,
            kyc_level: initial_kyc_level,
        }];
        let response = self
            .http
            .post(url)
            .header("apikey", &self.service_role_key)
            .header(AUTHORIZATION, format!("Bearer {}", self.service_role_key))
            .header(CONTENT_TYPE, "application/json")
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("supabase ensure user request failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "supabase ensure user failed: status={status} body={body}"
            ));
        }
        Ok(())
    }

    pub async fn fetch_user_kyc_state(
        &self,
        address_l2: &str,
    ) -> Result<Option<UserKycState>, String> {
        let mut url = reqwest::Url::parse(&format!("{}/users", self.rest_url))
            .map_err(|e| format!("supabase users url parse failed: {e}"))?;
        url.query_pairs_mut()
            .append_pair("select", "tx_count,kyc_level")
            .append_pair("address_l2", &format!("eq.{address_l2}"))
            .append_pair("limit", "1");
        let response = self
            .http
            .get(url)
            .header("apikey", &self.service_role_key)
            .header(AUTHORIZATION, format!("Bearer {}", self.service_role_key))
            .send()
            .await
            .map_err(|e| format!("supabase fetch user request failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "supabase fetch user failed: status={status} body={body}"
            ));
        }
        let rows: Vec<UserKycStateRow> = response
            .json()
            .await
            .map_err(|e| format!("supabase fetch user decode failed: {e}"))?;
        Ok(rows.into_iter().next().map(UserKycState::from))
    }

    pub async fn increment_user_tx_count(&self, address_l2: &str) -> Result<UserKycState, String> {
        let current = self
            .fetch_user_kyc_state(address_l2)
            .await?
            .unwrap_or(UserKycState {
                tx_count: 0,
                kyc_level: BASIC_KYC_LEVEL.to_string(),
            });
        let next_count = current.tx_count.saturating_add(1);
        self.patch_user_state(
            address_l2,
            UserStatePatch {
                tx_count: Some(next_count as i32),
                kyc_level: None,
            },
        )
        .await?;
        Ok(UserKycState {
            tx_count: next_count,
            kyc_level: current.kyc_level,
        })
    }

    pub async fn update_user_kyc_level(
        &self,
        address_l2: &str,
        kyc_level: &str,
    ) -> Result<(), String> {
        self.patch_user_state(
            address_l2,
            UserStatePatch {
                tx_count: None,
                kyc_level: Some(kyc_level),
            },
        )
        .await
    }

    pub async fn record_transaction(
        &self,
        record: &TransactionAuditRecord<'_>,
    ) -> Result<(), String> {
        let metadata = serde_json::from_str::<serde_json::Value>(record.metadata_json)
            .unwrap_or_else(|_| serde_json::json!({ "raw": record.metadata_json }));
        let payload = [TransactionInsertPayload {
            address_l2: record.address_l2,
            account_id: record.account_id,
            transaction_kind: record.transaction_kind,
            status: record.status,
            amount_minor: record.amount_minor as i64,
            currency: record.currency,
            provider: record.provider,
            reference_id: record.reference_id,
            metadata,
        }];
        let response = self
            .http
            .post(format!("{}/transactions", self.rest_url))
            .header("apikey", &self.service_role_key)
            .header(AUTHORIZATION, format!("Bearer {}", self.service_role_key))
            .header(CONTENT_TYPE, "application/json")
            .header("Prefer", "return=minimal")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("supabase transaction insert request failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "supabase transaction insert failed: status={status} body={body}"
            ));
        }
        Ok(())
    }

    async fn patch_user_state(
        &self,
        address_l2: &str,
        patch: UserStatePatch<'_>,
    ) -> Result<(), String> {
        let mut url = reqwest::Url::parse(&format!("{}/users", self.rest_url))
            .map_err(|e| format!("supabase users url parse failed: {e}"))?;
        url.query_pairs_mut()
            .append_pair("address_l2", &format!("eq.{address_l2}"));
        let response = self
            .http
            .patch(url)
            .header("apikey", &self.service_role_key)
            .header(AUTHORIZATION, format!("Bearer {}", self.service_role_key))
            .header(CONTENT_TYPE, "application/json")
            .header("Prefer", "return=minimal")
            .json(&patch)
            .send()
            .await
            .map_err(|e| format!("supabase patch request failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "supabase patch failed: status={status} body={body}"
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct UserKycStateRow {
    tx_count: Option<i32>,
    kyc_level: Option<String>,
}

impl From<UserKycStateRow> for UserKycState {
    fn from(value: UserKycStateRow) -> Self {
        Self {
            tx_count: value.tx_count.unwrap_or_default().max(0) as u64,
            kyc_level: value
                .kyc_level
                .unwrap_or_else(|| BASIC_KYC_LEVEL.to_string()),
        }
    }
}

pub fn is_basic_kyc(level: &str) -> bool {
    level.eq_ignore_ascii_case(BASIC_KYC_LEVEL)
}

pub fn verified_kyc_level() -> &'static str {
    VERIFIED_KYC_LEVEL
}

pub fn basic_kyc_level() -> &'static str {
    BASIC_KYC_LEVEL
}
