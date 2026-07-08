use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read};
use std::path::Path;

use crate::{
    Bandwidth, CodingRate, Context, HeaderMode, Modulation, PhyProfile, ProfilePlanner,
    SpreadingFactor,
};

const RAMAN_BINARY_MAGIC: &[u8] = b"RAMANB1\n";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanArtifactMetadata {
    pub artifact_version: String,
    pub source_kind: String,
    pub source_name: String,
    pub target: String,
    pub runtime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanRuleIr {
    pub priority: i32,
    pub order: usize,
    pub action_kind: String,
    pub exact_filters: Vec<[String; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanProgramIr {
    pub total_rules: usize,
    pub indexed_fields: Vec<String>,
    pub rules: Vec<RamanRuleIr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanRuntimeArtifact {
    pub metadata: RamanArtifactMetadata,
    pub compiled_rpl: String,
    pub ir: RamanProgramIr,
    #[serde(default)]
    pub executable: RamanExecutableProgram,
    #[serde(default)]
    pub device_abi: RamanDeviceAbiProgram,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct RamanExecutableProgram {
    pub rules: Vec<RamanExecutableRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanExecutableRule {
    pub priority: i32,
    pub order: usize,
    pub predicates: Vec<RamanExecutablePredicate>,
    pub action: RamanExecutableAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanExecutablePredicate {
    pub field: String,
    pub op: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanExecutableAction {
    pub kind: String,
    pub values: Vec<[String; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct RamanDeviceAbiProgram {
    pub version: String,
    pub rules: Vec<RamanDeviceAbiRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanDeviceAbiRule {
    pub priority: i32,
    pub order: usize,
    pub predicates: Vec<RamanDeviceAbiPredicate>,
    pub action: RamanDeviceAbiAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanDeviceAbiPredicate {
    pub field_id: u32,
    pub op_id: u32,
    pub value_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanDeviceAbiAction {
    pub action_id: u32,
    pub values: Vec<(u32, u32)>,
}

fn string_to_id<T: AsRef<str>>(s: T) -> u32 {
    let s = s.as_ref();
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish() as u32
}

fn parse_value_id<E>(value: serde_json::Value) -> Result<u32, E>
where
    E: serde::de::Error,
{
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .map(|v| v as u32)
            .ok_or_else(|| E::custom("invalid numeric value_id")),
        serde_json::Value::String(s) => Ok(string_to_id(s)),
        other => Err(E::custom(format!("invalid value type: {other:?}"))),
    }
}

impl<'de> serde::Deserialize<'de> for RamanDeviceAbiPredicate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Helper {
            field_id: u32,
            op_id: u32,
            value: serde_json::Value,
        }
        let helper = Helper::deserialize(deserializer)?;
        Ok(Self {
            field_id: helper.field_id,
            op_id: helper.op_id,
            value_id: parse_value_id(helper.value)?,
        })
    }
}

impl<'de> serde::Deserialize<'de> for RamanDeviceAbiAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Helper {
            action_id: u32,
            values: Vec<(serde_json::Value, serde_json::Value)>,
        }
        let helper = Helper::deserialize(deserializer)?;
        let mut values = Vec::with_capacity(helper.values.len());
        for (key, val) in helper.values {
            let key_id = parse_value_id(key)?;
            let value_id = parse_value_id(val)?;
            values.push((key_id, value_id));
        }
        Ok(Self {
            action_id: helper.action_id,
            values,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanArtifactValidation {
    pub total_rules: usize,
    pub indexed_fields: Vec<String>,
    pub traffic_candidate_rules: usize,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanRuntimeContext {
    pub minute: i32,
    pub traffic_class: String,
    pub hardware: String,
    pub scenario: String,
    pub noise_floor_dbm: i16,
    pub snr_db: f32,
    pub density: i32,
    pub latency_budget_ms: i32,
    pub battery_mv: i32,
    pub link_margin_db: f32,
    pub confidence: f32,
    pub predicted: String,
    pub drift_db: f32,
    pub renegotiation_needed: bool,
    pub rx_pdr: f32,
    pub error_rate: f32,
    pub path_stability: f32,
    pub memory_confidence: f32,
    pub worst_hop_reliability: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanRuntimeRis {
    pub mode: String,
    pub surface: String,
    pub azimuth_deg: i32,
    pub phase_profile: String,
    pub reflection_gain_db: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanRuntimeVerify {
    pub metric: String,
    pub min_value: f32,
    pub fallback: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanRuntimeSnapshot {
    pub profile_name: String,
    pub phy: PhyProfile,
    pub transport_priority: Option<String>,
    pub ris: Option<RamanRuntimeRis>,
    pub verify: Option<RamanRuntimeVerify>,
    pub contract_values: Vec<(String, String)>,
    pub hold_ticks: Option<u32>,
    pub cooldown_ticks: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanRuntimeResult {
    pub priority: String,
    pub snapshot: RamanRuntimeSnapshot,
    pub temporal_mode: Option<String>,
    pub temporal_reused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanExecutorState {
    temporal: Option<RamanExecutorTemporalState>,
}

#[derive(Debug, Clone, PartialEq)]
struct RamanExecutorTemporalState {
    stream_id: String,
    snapshot: RamanRuntimeSnapshot,
    priority: String,
    remaining_ticks: u32,
    mode: String,
}

#[derive(Debug, Clone)]
pub struct RamanMirrorExecutor {
    rules: Vec<MirrorRule>,
}

#[derive(Debug, Clone)]
struct MirrorRule {
    priority: i32,
    order: usize,
    predicates: Vec<MirrorPredicate>,
    action: MirrorAction,
}

#[derive(Debug, Clone)]
struct MirrorPredicate {
    field: String,
    op: String,
    value: String,
}

#[derive(Debug, Clone)]
struct MirrorAction {
    kind: String,
    values: Vec<(String, String)>,
}

#[derive(Debug)]
pub enum RamanArtifactError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    Invalid(&'static str),
    RuntimeParse(String),
}

impl core::fmt::Display for RamanArtifactError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Parse(err) => write!(f, "{err}"),
            Self::Invalid(msg) => write!(f, "{msg}"),
            Self::RuntimeParse(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RamanArtifactError {}

impl From<std::io::Error> for RamanArtifactError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for RamanArtifactError {
    fn from(value: serde_json::Error) -> Self {
        Self::Parse(value)
    }
}

pub fn load_raman_artifact(
    path: impl AsRef<Path>,
) -> Result<RamanRuntimeArtifact, RamanArtifactError> {
    let raw = fs::read_to_string(path)?;
    parse_raman_artifact(&raw)
}

pub fn load_raman_artifact_binary(
    path: impl AsRef<Path>,
) -> Result<RamanRuntimeArtifact, RamanArtifactError> {
    let raw = fs::read(path)?;
    parse_raman_artifact_binary(&raw)
}

pub fn parse_raman_artifact(raw: &str) -> Result<RamanRuntimeArtifact, RamanArtifactError> {
    let artifact: RamanRuntimeArtifact = serde_json::from_str(raw)?;
    validate_raman_artifact(&artifact)?;
    Ok(artifact)
}

pub fn encode_raman_artifact_binary(artifact: &RamanRuntimeArtifact) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(RAMAN_BINARY_MAGIC);
    write_string(&mut out, &artifact.metadata.artifact_version);
    write_string(&mut out, &artifact.metadata.source_kind);
    write_string(&mut out, &artifact.metadata.source_name);
    write_string(&mut out, &artifact.metadata.target);
    write_string(&mut out, &artifact.metadata.runtime);
    write_string(&mut out, &artifact.compiled_rpl);
    out.extend_from_slice(&(artifact.ir.total_rules as u32).to_le_bytes());
    out.extend_from_slice(&(artifact.ir.indexed_fields.len() as u32).to_le_bytes());
    for field in &artifact.ir.indexed_fields {
        write_string(&mut out, field);
    }
    out.extend_from_slice(&(artifact.ir.rules.len() as u32).to_le_bytes());
    for rule in &artifact.ir.rules {
        out.extend_from_slice(&rule.priority.to_le_bytes());
        out.extend_from_slice(&(rule.order as u32).to_le_bytes());
        write_string(&mut out, &rule.action_kind);
        out.extend_from_slice(&(rule.exact_filters.len() as u32).to_le_bytes());
        for pair in &rule.exact_filters {
            write_string(&mut out, &pair[0]);
            write_string(&mut out, &pair[1]);
        }
    }
    out.extend_from_slice(&(artifact.executable.rules.len() as u32).to_le_bytes());
    for rule in &artifact.executable.rules {
        out.extend_from_slice(&rule.priority.to_le_bytes());
        out.extend_from_slice(&(rule.order as u32).to_le_bytes());
        out.extend_from_slice(&(rule.predicates.len() as u32).to_le_bytes());
        for predicate in &rule.predicates {
            write_string(&mut out, &predicate.field);
            write_string(&mut out, &predicate.op);
            write_string(&mut out, &predicate.value);
        }
        write_string(&mut out, &rule.action.kind);
        out.extend_from_slice(&(rule.action.values.len() as u32).to_le_bytes());
        for pair in &rule.action.values {
            write_string(&mut out, &pair[0]);
            write_string(&mut out, &pair[1]);
        }
    }
    write_string(&mut out, &artifact.device_abi.version);
    out.extend_from_slice(&(artifact.device_abi.rules.len() as u32).to_le_bytes());
    for rule in &artifact.device_abi.rules {
        out.extend_from_slice(&rule.priority.to_le_bytes());
        out.extend_from_slice(&(rule.order as u32).to_le_bytes());
        out.extend_from_slice(&(rule.predicates.len() as u32).to_le_bytes());
        for predicate in &rule.predicates {
            out.extend_from_slice(&predicate.field_id.to_le_bytes());
            out.extend_from_slice(&predicate.op_id.to_le_bytes());
            out.extend_from_slice(&predicate.value_id.to_le_bytes());
        }
        out.extend_from_slice(&rule.action.action_id.to_le_bytes());
        out.extend_from_slice(&(rule.action.values.len() as u32).to_le_bytes());
        for pair in &rule.action.values {
            out.extend_from_slice(&pair.0.to_le_bytes());
            out.extend_from_slice(&pair.1.to_le_bytes());
        }
    }
    out
}

pub fn parse_raman_artifact_binary(raw: &[u8]) -> Result<RamanRuntimeArtifact, RamanArtifactError> {
    let mut cursor = Cursor::new(raw);
    let mut magic = vec![0_u8; RAMAN_BINARY_MAGIC.len()];
    cursor.read_exact(&mut magic)?;
    if magic.as_slice() != RAMAN_BINARY_MAGIC {
        return Err(RamanArtifactError::Invalid(
            "unsupported Raman binary artifact header",
        ));
    }
    let artifact = RamanRuntimeArtifact {
        metadata: RamanArtifactMetadata {
            artifact_version: read_string(&mut cursor)?,
            source_kind: read_string(&mut cursor)?,
            source_name: read_string(&mut cursor)?,
            target: read_string(&mut cursor)?,
            runtime: read_string(&mut cursor)?,
        },
        compiled_rpl: read_string(&mut cursor)?,
        ir: RamanProgramIr {
            total_rules: read_u32(&mut cursor)? as usize,
            indexed_fields: {
                let count = read_u32(&mut cursor)? as usize;
                let mut fields = Vec::with_capacity(count);
                for _ in 0..count {
                    fields.push(read_string(&mut cursor)?);
                }
                fields
            },
            rules: {
                let count = read_u32(&mut cursor)? as usize;
                let mut rules = Vec::with_capacity(count);
                for _ in 0..count {
                    let priority = read_i32(&mut cursor)?;
                    let order = read_u32(&mut cursor)? as usize;
                    let action_kind = read_string(&mut cursor)?;
                    let filter_count = read_u32(&mut cursor)? as usize;
                    let mut exact_filters = Vec::with_capacity(filter_count);
                    for _ in 0..filter_count {
                        exact_filters.push([read_string(&mut cursor)?, read_string(&mut cursor)?]);
                    }
                    rules.push(RamanRuleIr {
                        priority,
                        order,
                        action_kind,
                        exact_filters,
                    });
                }
                rules
            },
        },
        executable: {
            let count = read_u32(&mut cursor)? as usize;
            let mut rules = Vec::with_capacity(count);
            for _ in 0..count {
                let priority = read_i32(&mut cursor)?;
                let order = read_u32(&mut cursor)? as usize;
                let predicate_count = read_u32(&mut cursor)? as usize;
                let mut predicates = Vec::with_capacity(predicate_count);
                for _ in 0..predicate_count {
                    predicates.push(RamanExecutablePredicate {
                        field: read_string(&mut cursor)?,
                        op: read_string(&mut cursor)?,
                        value: read_string(&mut cursor)?,
                    });
                }
                let action_kind = read_string(&mut cursor)?;
                let values_count = read_u32(&mut cursor)? as usize;
                let mut values = Vec::with_capacity(values_count);
                for _ in 0..values_count {
                    values.push([read_string(&mut cursor)?, read_string(&mut cursor)?]);
                }
                rules.push(RamanExecutableRule {
                    priority,
                    order,
                    predicates,
                    action: RamanExecutableAction {
                        kind: action_kind,
                        values,
                    },
                });
            }
            RamanExecutableProgram { rules }
        },
        device_abi: if (cursor.position() as usize) < raw.len() {
            let version = read_string(&mut cursor)?;
            let count = read_u32(&mut cursor)? as usize;
            let mut rules = Vec::with_capacity(count);
            for _ in 0..count {
                let priority = read_i32(&mut cursor)?;
                let order = read_u32(&mut cursor)? as usize;
                let predicate_count = read_u32(&mut cursor)? as usize;
                let mut predicates = Vec::with_capacity(predicate_count);
                for _ in 0..predicate_count {
                    predicates.push(RamanDeviceAbiPredicate {
                        field_id: read_u32(&mut cursor)?,
                        op_id: read_u32(&mut cursor)?,
                        value_id: read_u32(&mut cursor)?,
                    });
                }
                let action_id = read_u32(&mut cursor)?;
                let values_count = read_u32(&mut cursor)? as usize;
                let mut values = Vec::with_capacity(values_count);
                for _ in 0..values_count {
                    let key_id = read_u32(&mut cursor)?;
                    let value_id = read_u32(&mut cursor)?;
                    values.push((key_id, value_id));
                }
                rules.push(RamanDeviceAbiRule {
                    priority,
                    order,
                    predicates,
                    action: RamanDeviceAbiAction { action_id, values },
                });
            }
            RamanDeviceAbiProgram { version, rules }
        } else {
            RamanDeviceAbiProgram::default()
        },
    };
    validate_raman_artifact(&artifact)?;
    Ok(artifact)
}

pub fn validate_raman_artifact(
    artifact: &RamanRuntimeArtifact,
) -> Result<RamanArtifactValidation, RamanArtifactError> {
    if artifact.metadata.artifact_version != "raman-artifact/v1" {
        return Err(RamanArtifactError::Invalid(
            "unsupported Raman artifact version",
        ));
    }
    if artifact.metadata.runtime != "RamanExecutor v1" {
        return Err(RamanArtifactError::Invalid("unsupported Raman runtime"));
    }
    if artifact.compiled_rpl.trim().is_empty() {
        return Err(RamanArtifactError::Invalid(
            "compiled RPL must not be empty",
        ));
    }
    if artifact.ir.total_rules == 0 || artifact.ir.rules.is_empty() {
        return Err(RamanArtifactError::Invalid(
            "IR must contain at least one rule",
        ));
    }
    if artifact.ir.total_rules != artifact.ir.rules.len() {
        return Err(RamanArtifactError::Invalid("IR rule count mismatch"));
    }
    if artifact.ir.indexed_fields.is_empty() {
        return Err(RamanArtifactError::Invalid(
            "IR indexed fields must not be empty",
        ));
    }
    if !artifact.executable.rules.is_empty()
        && artifact.executable.rules.len() != artifact.ir.total_rules
    {
        return Err(RamanArtifactError::Invalid(
            "executable rule count mismatch",
        ));
    }
    if !artifact.device_abi.rules.is_empty() {
        if artifact.device_abi.version != "raman-device-abi/v1" {
            return Err(RamanArtifactError::Invalid(
                "unsupported Raman device ABI version",
            ));
        }
        if artifact.device_abi.rules.len() != artifact.ir.total_rules {
            return Err(RamanArtifactError::Invalid(
                "device ABI rule count mismatch",
            ));
        }
    }
    let traffic_candidate_rules = candidate_rule_count(
        artifact,
        Some("truth_critical"),
        Some("sx1262_lab"),
        Some("industrial_shift"),
    );
    Ok(RamanArtifactValidation {
        total_rules: artifact.ir.total_rules,
        indexed_fields: artifact.ir.indexed_fields.clone(),
        traffic_candidate_rules,
        target: artifact.metadata.target.clone(),
    })
}

pub fn candidate_rule_count(
    artifact: &RamanRuntimeArtifact,
    traffic: Option<&str>,
    hardware: Option<&str>,
    scenario: Option<&str>,
) -> usize {
    artifact
        .ir
        .rules
        .iter()
        .filter(|rule| {
            exact_filter_matches(&rule.exact_filters, "traffic", traffic)
                && exact_filter_matches(&rule.exact_filters, "hardware", hardware)
                && exact_filter_matches(&rule.exact_filters, "scenario", scenario)
        })
        .count()
}

impl RamanRuntimeContext {
    pub fn from_core(
        context: &Context,
        minute: i32,
        traffic_class: impl Into<String>,
        hardware: impl Into<String>,
        scenario: impl Into<String>,
        density: i32,
    ) -> Self {
        let snr_db = context.snr_db as f32;
        let link_margin_db = context.link_margin_db as f32;
        let battery_mv = context.battery_mv as i32;
        let latency_budget_ms = context.latency_budget_ms as i32;
        let path_stability = clamp(
            0.52 + snr_db.max(-6.0) * 0.03 + link_margin_db.max(0.0) * 0.02
                - density as f32 * 0.006,
            0.05,
            0.98,
        );
        let rx_pdr = clamp(
            0.48 + snr_db.max(-6.0) * 0.035 + link_margin_db.max(0.0) * 0.018
                - density as f32 * 0.005,
            0.05,
            0.98,
        );
        let error_rate = clamp(
            0.24 + (2.0 - snr_db).max(0.0) * 0.045
                + (5.0 - link_margin_db).max(0.0) * 0.03
                + density as f32 * 0.0025,
            0.0,
            0.95,
        );
        let memory_confidence = clamp(
            0.58 + snr_db.max(-4.0) * 0.025 + link_margin_db.max(0.0) * 0.015
                - density as f32 * 0.003,
            0.10,
            0.96,
        );
        let worst_hop_reliability = clamp(
            path_stability * 0.72 + rx_pdr * 0.28 - error_rate * 0.18,
            0.05,
            0.98,
        );
        Self {
            minute,
            traffic_class: traffic_class.into(),
            hardware: hardware.into(),
            scenario: scenario.into(),
            noise_floor_dbm: context.noise_floor_dbm,
            snr_db,
            density,
            latency_budget_ms,
            battery_mv,
            link_margin_db,
            confidence: 0.75,
            predicted: "stable".to_string(),
            drift_db: 0.0,
            renegotiation_needed: snr_db <= 2.0 || link_margin_db <= 5.0,
            rx_pdr,
            error_rate,
            path_stability,
            memory_confidence,
            worst_hop_reliability,
        }
    }
}

impl RamanExecutorState {
    pub fn new() -> Self {
        Self { temporal: None }
    }
}

impl RamanMirrorExecutor {
    pub fn from_artifact(artifact: &RamanRuntimeArtifact) -> Result<Self, RamanArtifactError> {
        let rules = if artifact.executable.rules.is_empty() {
            parse_compiled_rpl_rules(&artifact.compiled_rpl)?
        } else {
            artifact
                .executable
                .rules
                .iter()
                .map(mirror_rule_from_executable)
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self { rules })
    }

    pub fn execute(
        &self,
        stream_id: &str,
        context: &RamanRuntimeContext,
        state: &mut RamanExecutorState,
    ) -> Result<RamanRuntimeResult, RamanArtifactError> {
        if let Some(mut active) = state.temporal.take() {
            if active.stream_id == stream_id && active.remaining_ticks > 0 {
                active.remaining_ticks -= 1;
                let result = RamanRuntimeResult {
                    priority: active.priority.clone(),
                    snapshot: active.snapshot.clone(),
                    temporal_mode: Some(active.mode.clone()),
                    temporal_reused: true,
                };
                if active.remaining_ticks > 0 {
                    state.temporal = Some(active);
                }
                return Ok(result);
            }
            state.temporal = Some(active);
        }

        let planner = ProfilePlanner::default();
        let base_phy = planner.generate_phy(&Context {
            noise_floor_dbm: context.noise_floor_dbm,
            snr_db: context.snr_db.round() as i8,
            battery_mv: context.battery_mv.clamp(0, u16::MAX as i32) as u16,
            latency_budget_ms: context.latency_budget_ms.max(0) as u32,
            link_margin_db: context.link_margin_db.round() as i8,
        });
        let mut snapshot = RamanRuntimeSnapshot {
            profile_name: profile_name_for_preset(base_phy),
            phy: base_phy,
            transport_priority: None,
            ris: None,
            verify: None,
            contract_values: Vec::new(),
            hold_ticks: None,
            cooldown_ticks: None,
        };

        for rule in &self.rules {
            if !matches_all(&rule.predicates, context) {
                continue;
            }
            match rule.action.kind.as_str() {
                "contract" if snapshot.contract_values.is_empty() => {
                    snapshot.contract_values = rule.action.values.clone();
                }
                "phy" if snapshot.phy == base_phy => {
                    snapshot.profile_name = value_for(&rule.action.values, "name")
                        .unwrap_or("unnamed")
                        .to_string();
                    snapshot.phy = phy_from_action(&rule.action.values)?;
                }
                "transport" if snapshot.transport_priority.is_none() => {
                    snapshot.transport_priority =
                        value_for(&rule.action.values, "priority").map(str::to_string);
                }
                "ris" if snapshot.ris.is_none() => {
                    snapshot.ris = Some(RamanRuntimeRis {
                        mode: required_value(&rule.action.values, "mode")?.to_string(),
                        surface: required_value(&rule.action.values, "surface")?.to_string(),
                        azimuth_deg: parse_i32(required_value(
                            &rule.action.values,
                            "azimuth_deg",
                        )?)?,
                        phase_profile: required_value(&rule.action.values, "phase_profile")?
                            .to_string(),
                        reflection_gain_db: required_value(
                            &rule.action.values,
                            "reflection_gain_db",
                        )?
                        .to_string(),
                    });
                }
                "verify" if snapshot.verify.is_none() => {
                    snapshot.verify = Some(RamanRuntimeVerify {
                        metric: required_value(&rule.action.values, "metric")?.to_string(),
                        min_value: parse_f32(required_value(&rule.action.values, "min")?)?,
                        fallback: required_value(&rule.action.values, "fallback")?.to_string(),
                    });
                }
                "hold" if snapshot.hold_ticks.is_none() => {
                    snapshot.hold_ticks =
                        Some(parse_u32(required_value(&rule.action.values, "ticks")?)?);
                }
                "cooldown" if snapshot.cooldown_ticks.is_none() => {
                    snapshot.cooldown_ticks =
                        Some(parse_u32(required_value(&rule.action.values, "ticks")?)?);
                }
                _ => {}
            }
        }

        let mut priority = snapshot
            .transport_priority
            .clone()
            .unwrap_or_else(|| default_priority(&context.traffic_class, &snapshot.profile_name));
        if let Some(verify) = &snapshot.verify {
            let metric_value = metric_value(context, &verify.metric);
            if metric_value < verify.min_value {
                priority = verify.fallback.clone();
            }
        }
        let mut temporal_mode = None;
        if let Some(ticks) = snapshot.hold_ticks {
            if ticks > 0 {
                state.temporal = Some(RamanExecutorTemporalState {
                    stream_id: stream_id.to_string(),
                    snapshot: snapshot.clone(),
                    priority: priority.clone(),
                    remaining_ticks: ticks,
                    mode: "hold".to_string(),
                });
                temporal_mode = Some("hold".to_string());
            }
        } else if let Some(ticks) = snapshot.cooldown_ticks {
            if ticks > 0 {
                state.temporal = Some(RamanExecutorTemporalState {
                    stream_id: stream_id.to_string(),
                    snapshot: snapshot.clone(),
                    priority: priority.clone(),
                    remaining_ticks: ticks,
                    mode: "cooldown".to_string(),
                });
                temporal_mode = Some("cooldown".to_string());
            }
        }

        Ok(RamanRuntimeResult {
            priority,
            snapshot,
            temporal_mode,
            temporal_reused: false,
        })
    }
}

fn exact_filter_matches(filters: &[[String; 2]], field: &str, requested: Option<&str>) -> bool {
    let values: Vec<&str> = filters
        .iter()
        .filter(|pair| pair[0] == field)
        .map(|pair| pair[1].as_str())
        .collect();
    if values.is_empty() {
        return true;
    }
    match requested {
        Some(value) => values.iter().any(|candidate| *candidate == value),
        None => false,
    }
}

fn parse_compiled_rpl_rules(raw: &str) -> Result<Vec<MirrorRule>, RamanArtifactError> {
    let mut rules = Vec::new();
    for (order, raw_line) in raw.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut priority = 100;
        let line_body = if let Some(rest) = line.strip_prefix("priority ") {
            let (priority_text, tail) = rest.split_once(" when ").ok_or_else(|| {
                RamanArtifactError::RuntimeParse(format!("invalid rule header: {line}"))
            })?;
            priority = parse_i32(priority_text)?;
            tail
        } else if let Some(rest) = line.strip_prefix("when ") {
            rest
        } else {
            return Err(RamanArtifactError::RuntimeParse(format!(
                "invalid rule body: {line}"
            )));
        };
        let (predicate_text, action_text) = line_body.split_once("->").ok_or_else(|| {
            RamanArtifactError::RuntimeParse(format!("missing action arrow: {line}"))
        })?;
        let predicates = predicate_text
            .split(" and ")
            .map(|token| parse_predicate(token.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        let action = parse_action(action_text.trim())?;
        rules.push(MirrorRule {
            priority,
            order,
            predicates,
            action,
        });
    }
    rules.sort_by_key(|rule| (-rule.priority, rule.order as i32));
    Ok(rules)
}

fn parse_predicate(token: &str) -> Result<MirrorPredicate, RamanArtifactError> {
    for op in [">=", "<=", "==", ">", "<"] {
        if let Some((field, value)) = token.split_once(op) {
            return Ok(MirrorPredicate {
                field: field.trim().to_string(),
                op: op.to_string(),
                value: value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            });
        }
    }
    Err(RamanArtifactError::RuntimeParse(format!(
        "invalid predicate: {token}"
    )))
}

fn parse_action(raw: &str) -> Result<MirrorAction, RamanArtifactError> {
    let (kind, tail) = raw
        .split_once('(')
        .ok_or_else(|| RamanArtifactError::RuntimeParse(format!("invalid action: {raw}")))?;
    let payload = tail.strip_suffix(')').ok_or_else(|| {
        RamanArtifactError::RuntimeParse(format!("invalid action payload: {raw}"))
    })?;
    let mut values = Vec::new();
    if !payload.trim().is_empty() {
        for pair in payload.split(",") {
            let (key, value) = pair.split_once('=').ok_or_else(|| {
                RamanArtifactError::RuntimeParse(format!("invalid action pair: {pair}"))
            })?;
            values.push((
                key.trim().to_string(),
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            ));
        }
    }
    Ok(MirrorAction {
        kind: kind.trim().to_string(),
        values,
    })
}

fn mirror_rule_from_executable(
    rule: &RamanExecutableRule,
) -> Result<MirrorRule, RamanArtifactError> {
    Ok(MirrorRule {
        priority: rule.priority,
        order: rule.order,
        predicates: rule
            .predicates
            .iter()
            .map(|predicate| MirrorPredicate {
                field: predicate.field.clone(),
                op: predicate.op.clone(),
                value: predicate.value.clone(),
            })
            .collect(),
        action: MirrorAction {
            kind: rule.action.kind.clone(),
            values: rule
                .action
                .values
                .iter()
                .map(|pair| (pair[0].clone(), pair[1].clone()))
                .collect(),
        },
    })
}

fn matches_all(predicates: &[MirrorPredicate], context: &RamanRuntimeContext) -> bool {
    predicates
        .iter()
        .all(|predicate| matches_predicate(predicate, context))
}

fn matches_predicate(predicate: &MirrorPredicate, context: &RamanRuntimeContext) -> bool {
    match predicate.field.as_str() {
        "minute" => compare_f32(
            context.minute as f32,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "traffic" => compare_str(
            &context.traffic_class,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "hardware" => compare_str(&context.hardware, &predicate.op, predicate.value.as_str()),
        "scenario" => compare_str(&context.scenario, &predicate.op, predicate.value.as_str()),
        "snr" => compare_f32(context.snr_db, &predicate.op, predicate.value.as_str()),
        "density" => compare_f32(
            context.density as f32,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "latency_budget_ms" => compare_f32(
            context.latency_budget_ms as f32,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "battery_mv" => compare_f32(
            context.battery_mv as f32,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "link_margin_db" => compare_f32(
            context.link_margin_db,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "confidence" => compare_f32(context.confidence, &predicate.op, predicate.value.as_str()),
        "predicted" => compare_str(&context.predicted, &predicate.op, predicate.value.as_str()),
        "drift_db" => compare_f32(context.drift_db, &predicate.op, predicate.value.as_str()),
        "renegotiation_needed" => compare_bool(
            context.renegotiation_needed,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "rx_pdr" => compare_f32(context.rx_pdr, &predicate.op, predicate.value.as_str()),
        "error_rate" => compare_f32(context.error_rate, &predicate.op, predicate.value.as_str()),
        "path_stability" => compare_f32(
            context.path_stability,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "memory_confidence" => compare_f32(
            context.memory_confidence,
            &predicate.op,
            predicate.value.as_str(),
        ),
        "worst_hop_reliability" => compare_f32(
            context.worst_hop_reliability,
            &predicate.op,
            predicate.value.as_str(),
        ),
        _ => false,
    }
}

fn compare_str(left: &str, op: &str, raw: &str) -> bool {
    op == "==" && left == raw
}

fn compare_bool(left: bool, op: &str, raw: &str) -> bool {
    op == "==" && left == (raw == "true")
}

fn compare_f32(left: f32, op: &str, raw: &str) -> bool {
    let Ok(right) = raw.parse::<f32>() else {
        return false;
    };
    match op {
        "==" => (left - right).abs() < 0.0001,
        ">" => left > right,
        "<" => left < right,
        ">=" => left >= right,
        "<=" => left <= right,
        _ => false,
    }
}

fn value_for<'a>(values: &'a [(String, String)], key: &str) -> Option<&'a str> {
    values.iter().find_map(|(current_key, value)| {
        if current_key == key {
            Some(value.as_str())
        } else {
            None
        }
    })
}

fn required_value<'a>(
    values: &'a [(String, String)],
    key: &str,
) -> Result<&'a str, RamanArtifactError> {
    value_for(values, key)
        .ok_or_else(|| RamanArtifactError::RuntimeParse(format!("missing action key: {key}")))
}

fn parse_i32(raw: &str) -> Result<i32, RamanArtifactError> {
    raw.parse::<i32>()
        .map_err(|_| RamanArtifactError::RuntimeParse(format!("invalid integer: {raw}")))
}

fn parse_u32(raw: &str) -> Result<u32, RamanArtifactError> {
    raw.parse::<u32>()
        .map_err(|_| RamanArtifactError::RuntimeParse(format!("invalid unsigned integer: {raw}")))
}

fn parse_f32(raw: &str) -> Result<f32, RamanArtifactError> {
    raw.parse::<f32>()
        .map_err(|_| RamanArtifactError::RuntimeParse(format!("invalid float: {raw}")))
}

fn phy_from_action(values: &[(String, String)]) -> Result<PhyProfile, RamanArtifactError> {
    let mut profile = PhyProfile::lora_default();
    profile.bandwidth = match required_value(values, "bandwidth_khz")? {
        "62" => Bandwidth::Khz62,
        "125" => Bandwidth::Khz125,
        "250" => Bandwidth::Khz250,
        "500" => Bandwidth::Khz500,
        raw => {
            return Err(RamanArtifactError::RuntimeParse(format!(
                "unsupported bandwidth_khz: {raw}"
            )))
        }
    };
    profile.spreading_factor = Some(match required_value(values, "spreading_factor")? {
        "7" => SpreadingFactor::Sf7,
        "8" => SpreadingFactor::Sf8,
        "9" => SpreadingFactor::Sf9,
        "10" => SpreadingFactor::Sf10,
        "11" => SpreadingFactor::Sf11,
        "12" => SpreadingFactor::Sf12,
        raw => {
            return Err(RamanArtifactError::RuntimeParse(format!(
                "unsupported spreading_factor: {raw}"
            )))
        }
    });
    profile.coding_rate = Some(match required_value(values, "coding_rate_denominator")? {
        "5" => CodingRate::Cr45,
        "6" => CodingRate::Cr46,
        "7" => CodingRate::Cr47,
        "8" => CodingRate::Cr48,
        raw => {
            return Err(RamanArtifactError::RuntimeParse(format!(
                "unsupported coding_rate_denominator: {raw}"
            )))
        }
    });
    profile.tx_power_dbm = parse_i32(required_value(values, "tx_power_dbm")?)? as i8;
    profile.preamble_len = parse_u32(required_value(values, "preamble_symbols")?)? as u16;
    if let Some(crc_enabled) = value_for(values, "crc_enabled") {
        profile.crc_enabled = crc_enabled == "true";
    }
    profile.modulation = Modulation::LoRa;
    profile.header_mode = HeaderMode::Explicit;
    Ok(profile)
}

fn profile_name_for_preset(profile: PhyProfile) -> String {
    match profile.preset() {
        Some(preset) => format!("preset::{preset:?}").to_lowercase(),
        None => "planner::derived".to_string(),
    }
}

fn default_priority(traffic_class: &str, profile_name: &str) -> String {
    if profile_name == "ril_control_guard_degrading" {
        return "command_guard".to_string();
    }
    if profile_name == "ril_sensor_uplink_guard" || profile_name == "ril_relay_bulk_guard" {
        return "bulk_guard".to_string();
    }
    if profile_name == "ril_low_observable_guard" {
        return "stealth_guard".to_string();
    }
    match traffic_class {
        "control_authoritative" | "truth_critical" => "critical",
        "voice_live" => "high",
        _ => "bulk",
    }
    .to_string()
}

fn metric_value(context: &RamanRuntimeContext, metric: &str) -> f32 {
    match metric {
        "path_stability" => context.path_stability,
        "rx_pdr" => context.rx_pdr,
        "error_rate" => context.error_rate,
        "memory_confidence" => context.memory_confidence,
        "worst_hop_reliability" => context.worst_hop_reliability,
        _ => 1.0,
    }
}

fn clamp(value: f32, min_value: f32, max_value: f32) -> f32 {
    value.max(min_value).min(max_value)
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, RamanArtifactError> {
    let mut raw = [0_u8; 4];
    cursor.read_exact(&mut raw)?;
    Ok(u32::from_le_bytes(raw))
}

fn read_i32(cursor: &mut Cursor<&[u8]>) -> Result<i32, RamanArtifactError> {
    let mut raw = [0_u8; 4];
    cursor.read_exact(&mut raw)?;
    Ok(i32::from_le_bytes(raw))
}

fn read_string(cursor: &mut Cursor<&[u8]>) -> Result<String, RamanArtifactError> {
    let length = read_u32(cursor)? as usize;
    let mut raw = vec![0_u8; length];
    cursor.read_exact(&mut raw)?;
    String::from_utf8(raw)
        .map_err(|_| RamanArtifactError::Invalid("invalid utf-8 in Raman binary artifact"))
}

fn write_string(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}
