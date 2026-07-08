use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{
    Bandwidth, RamanArtifactError, RamanDeviceAbiRule, RamanExecutorState, RamanMirrorExecutor,
    RamanRuntimeArtifact, RamanRuntimeContext, RamanRuntimeResult, SpreadingFactor,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RamanCapabilityDescriptor {
    pub vendor: String,
    pub model: String,
    pub chip_family: String,
    pub radio_chip: String,
    pub sf_min: u8,
    pub sf_max: u8,
    pub bw_set_khz: Vec<u16>,
    pub tx_power_max_dbm: i8,
    pub supports_sleep_wake: bool,
    pub irq_model: String,
    pub supports_ris: bool,
    pub supports_transport_priority: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanArtifactRequirements {
    pub sf_min: u8,
    pub sf_max: u8,
    pub bw_set_khz: Vec<u16>,
    pub tx_power_max_dbm: i8,
    pub requires_ris: bool,
    pub requires_transport_priority: bool,
    pub total_rules: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanAbiNegotiationReport {
    pub compatible: bool,
    pub downgraded: bool,
    pub negotiated_sf_min: u8,
    pub negotiated_sf_max: u8,
    pub negotiated_bw_set_khz: Vec<u16>,
    pub negotiated_tx_power_max_dbm: i8,
    pub negotiated_ris: bool,
    pub negotiated_transport_priority: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanRhmlRule {
    pub field: String,
    pub register: String,
    pub scale: i32,
    pub offset: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanRhmlProgram {
    pub rules: Vec<RamanRhmlRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRegisterWrite {
    pub register: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChipSelftestVector {
    pub name: String,
    pub expected_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamanChipPack {
    pub root: PathBuf,
    pub capability: RamanCapabilityDescriptor,
    pub rhml: RamanRhmlProgram,
    pub selftests: Vec<ChipSelftestVector>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortablePipelineReport {
    pub chip: String,
    pub decision: RamanRuntimeResult,
    pub requirements: RamanArtifactRequirements,
    pub negotiation: RamanAbiNegotiationReport,
    pub writes: Vec<PortableRegisterWrite>,
}

pub fn derive_artifact_requirements(artifact: &RamanRuntimeArtifact) -> RamanArtifactRequirements {
    let mut sf_min = u8::MAX;
    let mut sf_max = 0u8;
    let mut tx_power_max = i8::MIN;
    let mut bw_set = Vec::<u16>::new();
    let mut requires_ris = false;
    let mut requires_transport = false;

    for rule in &artifact.device_abi.rules {
        derive_from_device_rule(
            rule,
            &mut sf_min,
            &mut sf_max,
            &mut tx_power_max,
            &mut bw_set,
            &mut requires_ris,
            &mut requires_transport,
        );
    }

    if sf_min == u8::MAX {
        sf_min = 7;
    }
    if sf_max == 0 {
        sf_max = 12;
    }
    if tx_power_max == i8::MIN {
        tx_power_max = 20;
    }
    bw_set.sort_unstable();
    bw_set.dedup();
    if bw_set.is_empty() {
        bw_set = vec![62, 125, 250, 500];
    }

    RamanArtifactRequirements {
        sf_min,
        sf_max,
        bw_set_khz: bw_set,
        tx_power_max_dbm: tx_power_max,
        requires_ris,
        requires_transport_priority: requires_transport,
        total_rules: artifact.ir.total_rules,
    }
}

pub fn negotiate_artifact_with_capability(
    requirements: &RamanArtifactRequirements,
    capability: &RamanCapabilityDescriptor,
) -> RamanAbiNegotiationReport {
    let mut notes = Vec::new();
    let mut downgraded = false;

    let negotiated_sf_min = requirements.sf_min.max(capability.sf_min);
    let negotiated_sf_max = requirements.sf_max.min(capability.sf_max);
    let compatible = negotiated_sf_min <= negotiated_sf_max;
    if !compatible {
        notes.push(format!(
            "sf_range_incompatible artifact=[{}..{}] device=[{}..{}]",
            requirements.sf_min, requirements.sf_max, capability.sf_min, capability.sf_max
        ));
    } else if negotiated_sf_min != requirements.sf_min || negotiated_sf_max != requirements.sf_max {
        downgraded = true;
        notes.push(format!(
            "sf_range_downgraded to [{}..{}]",
            negotiated_sf_min, negotiated_sf_max
        ));
    }

    let mut negotiated_bw_set_khz = requirements
        .bw_set_khz
        .iter()
        .copied()
        .filter(|value| capability.bw_set_khz.contains(value))
        .collect::<Vec<_>>();
    if negotiated_bw_set_khz.is_empty() {
        if let Some(fallback_bw) = capability.bw_set_khz.first().copied() {
            negotiated_bw_set_khz.push(fallback_bw);
            downgraded = true;
            notes.push(format!(
                "bw_set_downgraded no_intersection fallback={fallback_bw}"
            ));
        }
    } else if negotiated_bw_set_khz.len() != requirements.bw_set_khz.len() {
        downgraded = true;
        notes.push(format!(
            "bw_set_reduced {} -> {}",
            requirements.bw_set_khz.len(),
            negotiated_bw_set_khz.len()
        ));
    }

    let negotiated_tx_power_max_dbm = requirements
        .tx_power_max_dbm
        .min(capability.tx_power_max_dbm);
    if negotiated_tx_power_max_dbm != requirements.tx_power_max_dbm {
        downgraded = true;
        notes.push(format!(
            "tx_power_downgraded {} -> {}",
            requirements.tx_power_max_dbm, negotiated_tx_power_max_dbm
        ));
    }

    let negotiated_ris = requirements.requires_ris && capability.supports_ris;
    if requirements.requires_ris && !capability.supports_ris {
        downgraded = true;
        notes.push("ris_disabled_by_capability".to_string());
    }

    let negotiated_transport_priority =
        requirements.requires_transport_priority && capability.supports_transport_priority;
    if requirements.requires_transport_priority && !capability.supports_transport_priority {
        downgraded = true;
        notes.push("transport_priority_disabled_by_capability".to_string());
    }

    RamanAbiNegotiationReport {
        compatible,
        downgraded,
        negotiated_sf_min,
        negotiated_sf_max,
        negotiated_bw_set_khz,
        negotiated_tx_power_max_dbm,
        negotiated_ris,
        negotiated_transport_priority,
        notes,
    }
}

pub fn load_chip_pack(root: impl AsRef<Path>) -> Result<RamanChipPack, RamanArtifactError> {
    let root = root.as_ref().to_path_buf();
    let capability_raw = fs::read_to_string(root.join("capability.json"))
        .map_err(|e| RamanArtifactError::RuntimeParse(format!("read capability.json: {e}")))?;
    let capability = serde_json::from_str::<RamanCapabilityDescriptor>(&capability_raw)
        .map_err(|e| RamanArtifactError::RuntimeParse(format!("parse capability.json: {e}")))?;

    let mapping_raw = fs::read_to_string(root.join("mapping.rhml"))
        .map_err(|e| RamanArtifactError::RuntimeParse(format!("read mapping.rhml: {e}")))?;
    let rhml = parse_rhml(&mapping_raw)?;

    let selftests = match fs::read_to_string(root.join("selftest_vectors.json")) {
        Ok(raw) => serde_json::from_str::<Vec<ChipSelftestVector>>(&raw).map_err(|e| {
            RamanArtifactError::RuntimeParse(format!("parse selftest_vectors.json: {e}"))
        })?,
        Err(_) => Vec::new(),
    };

    Ok(RamanChipPack {
        root,
        capability,
        rhml,
        selftests,
    })
}

pub fn compile_rhml_command_plan(
    decision: &RamanRuntimeResult,
    rhml: &RamanRhmlProgram,
    negotiation: &RamanAbiNegotiationReport,
) -> Vec<PortableRegisterWrite> {
    let mut writes = Vec::new();
    for rule in &rhml.rules {
        let mut value = match rule.field.as_str() {
            "bandwidth_khz" => bandwidth_khz(decision.snapshot.phy.bandwidth) as i32,
            "spreading_factor" => decision
                .snapshot
                .phy
                .spreading_factor
                .map(spreading_factor_numeric)
                .unwrap_or(0) as i32,
            "tx_power_dbm" => decision.snapshot.phy.tx_power_dbm as i32,
            _ => continue,
        };
        if rule.field == "spreading_factor" {
            value = value.clamp(
                negotiation.negotiated_sf_min as i32,
                negotiation.negotiated_sf_max as i32,
            );
        }
        if rule.field == "tx_power_dbm" {
            value = value.min(negotiation.negotiated_tx_power_max_dbm as i32);
        }
        if rule.field == "bandwidth_khz"
            && !negotiation
                .negotiated_bw_set_khz
                .iter()
                .any(|item| *item as i32 == value)
        {
            if let Some(fallback) = negotiation.negotiated_bw_set_khz.first() {
                value = *fallback as i32;
            }
        }
        let transformed = value.saturating_mul(rule.scale).saturating_add(rule.offset);
        writes.push(PortableRegisterWrite {
            register: rule.register.clone(),
            value: transformed.max(0) as u32,
        });
    }
    writes
}

pub fn run_portable_pipeline(
    artifact: &RamanRuntimeArtifact,
    context: &RamanRuntimeContext,
    stream_id: &str,
    chip_pack_root: impl AsRef<Path>,
) -> Result<PortablePipelineReport, RamanArtifactError> {
    let pack = load_chip_pack(chip_pack_root)?;
    let requirements = derive_artifact_requirements(artifact);
    let negotiation = negotiate_artifact_with_capability(&requirements, &pack.capability);
    if !negotiation.compatible {
        return Err(RamanArtifactError::RuntimeParse(
            "Raman ABI negotiation failed: incompatible capability".to_string(),
        ));
    }

    let executor = RamanMirrorExecutor::from_artifact(artifact)?;
    let mut state = RamanExecutorState::new();
    let decision = executor.execute(stream_id, context, &mut state)?;
    let writes = compile_rhml_command_plan(&decision, &pack.rhml, &negotiation);

    Ok(PortablePipelineReport {
        chip: format!("{}:{}", pack.capability.vendor, pack.capability.model),
        decision,
        requirements,
        negotiation,
        writes,
    })
}

fn derive_from_device_rule(
    rule: &RamanDeviceAbiRule,
    sf_min: &mut u8,
    sf_max: &mut u8,
    tx_power_max: &mut i8,
    bw_set: &mut Vec<u16>,
    requires_ris: &mut bool,
    requires_transport: &mut bool,
) {
    for (key, value) in &rule.action.values {
        if *key == raman_core::symbol_id("spreading_factor") {
            let sf = (*value as i32).clamp(7, 12) as u8;
            *sf_min = (*sf_min).min(sf);
            *sf_max = (*sf_max).max(sf);
        }
        if *key == raman_core::symbol_id("bandwidth_khz") {
            let bw = (*value).clamp(62, 500) as u16;
            if !bw_set.contains(&bw) {
                bw_set.push(bw);
            }
        }
        if *key == raman_core::symbol_id("tx_power_dbm") {
            let p = (*value as i32).clamp(-4, 20) as i8;
            *tx_power_max = (*tx_power_max).max(p);
        }
        if *key == raman_core::symbol_id("mode") || *key == raman_core::symbol_id("surface") {
            *requires_ris = true;
        }
        if *key == raman_core::symbol_id("priority") {
            *requires_transport = true;
        }
    }
}

fn parse_rhml(raw: &str) -> Result<RamanRhmlProgram, RamanArtifactError> {
    let mut rules = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.len() < 3 || tokens[0] != "map" {
            return Err(RamanArtifactError::RuntimeParse(format!(
                "invalid RHML line: {line}"
            )));
        }
        let mut scale = 1i32;
        let mut offset = 0i32;
        for token in tokens.iter().skip(3) {
            if let Some(raw) = token.strip_prefix("scale=") {
                scale = raw.parse::<i32>().map_err(|e| {
                    RamanArtifactError::RuntimeParse(format!("invalid RHML scale {raw}: {e}"))
                })?;
            } else if let Some(raw) = token.strip_prefix("offset=") {
                offset = raw.parse::<i32>().map_err(|e| {
                    RamanArtifactError::RuntimeParse(format!("invalid RHML offset {raw}: {e}"))
                })?;
            }
        }
        rules.push(RamanRhmlRule {
            field: tokens[1].to_string(),
            register: tokens[2].to_string(),
            scale,
            offset,
        });
    }
    Ok(RamanRhmlProgram { rules })
}

fn bandwidth_khz(value: Bandwidth) -> u16 {
    match value {
        Bandwidth::Khz62 => 62,
        Bandwidth::Khz125 => 125,
        Bandwidth::Khz250 => 250,
        Bandwidth::Khz500 => 500,
    }
}

fn spreading_factor_numeric(value: SpreadingFactor) -> u8 {
    match value {
        SpreadingFactor::Sf7 => 7,
        SpreadingFactor::Sf8 => 8,
        SpreadingFactor::Sf9 => 9,
        SpreadingFactor::Sf10 => 10,
        SpreadingFactor::Sf11 => 11,
        SpreadingFactor::Sf12 => 12,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bandwidth_khz, compile_rhml_command_plan, negotiate_artifact_with_capability, parse_rhml,
        RamanAbiNegotiationReport, RamanArtifactRequirements, RamanCapabilityDescriptor,
    };
    use crate::{
        Bandwidth, HeaderMode, Modulation, PhyProfile, RamanRuntimeResult, RamanRuntimeSnapshot,
    };

    #[test]
    fn parses_rhml_and_builds_plan() {
        let rhml = parse_rhml(
            r#"
            # map action fields to registers
            map bandwidth_khz REG_MODEM_CONFIG_1
            map spreading_factor REG_MODEM_CONFIG_2
            map tx_power_dbm REG_PA_CONFIG scale=1 offset=3
            "#,
        )
        .expect("RHML should parse");

        let decision = RamanRuntimeResult {
            priority: "priority_100".to_string(),
            snapshot: RamanRuntimeSnapshot {
                profile_name: "resilient".to_string(),
                phy: PhyProfile {
                    modulation: Modulation::LoRa,
                    frequency_hz: 868_100_000,
                    bandwidth: Bandwidth::Khz125,
                    spreading_factor: Some(crate::SpreadingFactor::Sf11),
                    coding_rate: Some(crate::CodingRate::Cr48),
                    preamble_len: 8,
                    sync_word: 0x12,
                    tx_power_dbm: 17,
                    crc_enabled: true,
                    whitening_enabled: false,
                    header_mode: HeaderMode::Explicit,
                },
                transport_priority: Some("critical".to_string()),
                ris: None,
                verify: None,
                contract_values: Vec::new(),
                hold_ticks: None,
                cooldown_ticks: None,
            },
            temporal_mode: None,
            temporal_reused: false,
        };
        let negotiation = RamanAbiNegotiationReport {
            compatible: true,
            downgraded: false,
            negotiated_sf_min: 7,
            negotiated_sf_max: 12,
            negotiated_bw_set_khz: vec![125, 250],
            negotiated_tx_power_max_dbm: 20,
            negotiated_ris: false,
            negotiated_transport_priority: true,
            notes: Vec::new(),
        };
        let writes = compile_rhml_command_plan(&decision, &rhml, &negotiation);
        assert_eq!(writes.len(), 3);
        assert_eq!(writes[0].register, "REG_MODEM_CONFIG_1");
        assert_eq!(writes[0].value, bandwidth_khz(Bandwidth::Khz125) as u32);
    }

    #[test]
    fn deterministic_negotiation_downgrades_capabilities() {
        let requirements = RamanArtifactRequirements {
            sf_min: 7,
            sf_max: 12,
            bw_set_khz: vec![125, 250],
            tx_power_max_dbm: 20,
            requires_ris: true,
            requires_transport_priority: true,
            total_rules: 4,
        };
        let capability = RamanCapabilityDescriptor {
            vendor: "vendor".to_string(),
            model: "chip".to_string(),
            chip_family: "mcu".to_string(),
            radio_chip: "sx127x".to_string(),
            sf_min: 8,
            sf_max: 11,
            bw_set_khz: vec![125],
            tx_power_max_dbm: 14,
            supports_sleep_wake: true,
            irq_model: "dio1".to_string(),
            supports_ris: false,
            supports_transport_priority: true,
        };
        let report = negotiate_artifact_with_capability(&requirements, &capability);
        assert!(report.compatible);
        assert!(report.downgraded);
        assert_eq!(report.negotiated_sf_min, 8);
        assert_eq!(report.negotiated_sf_max, 11);
        assert_eq!(report.negotiated_bw_set_khz, vec![125]);
        assert_eq!(report.negotiated_tx_power_max_dbm, 14);
        assert!(!report.negotiated_ris);
    }
}
