use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use radnet_morphic_kernel::{
    load_raman_artifact, load_raman_artifact_binary, run_portable_pipeline, RamanRuntimeContext,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GbcArtifact {
    #[allow(dead_code)]
    artifact_type: String,
    rules: Vec<GbcRule>,
}

#[derive(Debug, Deserialize)]
struct GbcRule {
    name: String,
    priority: i32,
    predicates: Vec<GbcPredicate>,
    action: GbcAction,
}

#[derive(Debug, Deserialize)]
struct GbcPredicate {
    field: String,
    op: String,
    #[serde(rename = "type")]
    value_type: String,
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GbcAction {
    kind: String,
    values: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct RhmlRule {
    field: String,
    register: String,
    scale: i32,
    offset: i32,
}

#[derive(Debug, Clone)]
enum PredValue {
    Num(f64),
    Bool(bool),
    Str(String),
}

#[derive(Debug, Clone)]
struct GbcBinPredicate {
    field: String,
    op: u8,
    value: PredValue,
}

#[derive(Debug, Clone)]
struct GbcBinRule {
    name: String,
    priority: i32,
    predicates: Vec<GbcBinPredicate>,
    action_kind: u8,
    action_values: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct GbcBinArtifact {
    rules: Vec<GbcBinRule>,
}

#[derive(Debug, Clone)]
struct Decision {
    matched: bool,
    rule_name: String,
    priority: i32,
    profile: String,
    writes: Vec<(String, u32)>,
}

fn parse_rhml_mappings(path: &PathBuf) -> Vec<RhmlRule> {
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut rules = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 3 || parts[0] != "map" {
            continue;
        }
        let mut scale = 1i32;
        let mut offset = 0i32;
        for token in parts.iter().skip(3) {
            if let Some(raw_scale) = token.strip_prefix("scale=") {
                if let Ok(v) = raw_scale.parse::<i32>() {
                    scale = v;
                }
            } else if let Some(raw_offset) = token.strip_prefix("offset=") {
                if let Ok(v) = raw_offset.parse::<i32>() {
                    offset = v;
                }
            }
        }
        rules.push(RhmlRule {
            field: parts[1].to_string(),
            register: parts[2].to_string(),
            scale,
            offset,
        });
    }
    rules
}

fn context_field_value(context: &RamanRuntimeContext, field: &str) -> Option<String> {
    match field {
        "traffic" => Some(context.traffic_class.clone()),
        "confidence" => Some(context.confidence.to_string()),
        "predicted" => Some(context.predicted.clone()),
        "renegotiation_needed" => Some(context.renegotiation_needed.to_string()),
        "snr" => Some(context.snr_db.to_string()),
        "density" => Some(context.density.to_string()),
        "latency_budget_ms" => Some(context.latency_budget_ms.to_string()),
        "battery_mv" => Some(context.battery_mv.to_string()),
        "link_margin_db" => Some(context.link_margin_db.to_string()),
        _ => None,
    }
}

fn predicate_matches_json(pred: &GbcPredicate, context: &RamanRuntimeContext) -> bool {
    let Some(raw_current) = context_field_value(context, &pred.field) else {
        return false;
    };
    match pred.value_type.as_str() {
        "num" => {
            let Ok(cur) = raw_current.parse::<f64>() else {
                return false;
            };
            let Some(target) = pred.value.as_f64() else {
                return false;
            };
            match pred.op.as_str() {
                "==" => cur == target,
                "!=" => cur != target,
                ">=" => cur >= target,
                "<=" => cur <= target,
                ">" => cur > target,
                "<" => cur < target,
                _ => false,
            }
        }
        "bool" => {
            let cur = raw_current.eq_ignore_ascii_case("true");
            let Some(target) = pred.value.as_bool() else {
                return false;
            };
            match pred.op.as_str() {
                "==" => cur == target,
                "!=" => cur != target,
                _ => false,
            }
        }
        _ => {
            let target = pred.value.as_str().unwrap_or_default().to_string();
            match pred.op.as_str() {
                "==" => raw_current == target,
                "!=" => raw_current != target,
                _ => false,
            }
        }
    }
}

fn predicate_matches_bin(pred: &GbcBinPredicate, context: &RamanRuntimeContext) -> bool {
    let Some(raw_current) = context_field_value(context, &pred.field) else {
        return false;
    };
    match &pred.value {
        PredValue::Num(target) => {
            let Ok(cur) = raw_current.parse::<f64>() else {
                return false;
            };
            match pred.op {
                0 => cur == *target,
                1 => cur != *target,
                2 => cur >= *target,
                3 => cur <= *target,
                4 => cur > *target,
                5 => cur < *target,
                _ => false,
            }
        }
        PredValue::Bool(target) => {
            let cur = raw_current.eq_ignore_ascii_case("true");
            match pred.op {
                0 => cur == *target,
                1 => cur != *target,
                _ => false,
            }
        }
        PredValue::Str(target) => match pred.op {
            0 => raw_current == *target,
            1 => raw_current != *target,
            _ => false,
        },
    }
}

fn apply_action_to_writes_with_mappings(
    action_values: &HashMap<String, String>,
    mappings: &[RhmlRule],
) -> Vec<(String, u32)> {
    let mut writes = Vec::<(String, u32)>::new();
    for map in mappings {
        let Some(raw_val) = action_values.get(&map.field) else {
            continue;
        };
        let Ok(parsed) = raw_val.parse::<i32>() else {
            continue;
        };
        let transformed = parsed
            .saturating_mul(map.scale)
            .saturating_add(map.offset)
            .max(0) as u32;
        writes.push((map.register.clone(), transformed));
    }
    writes
}

fn decision_to_bin(decision: &Decision) -> Vec<u8> {
    let mut out = Vec::<u8>::new();
    out.extend_from_slice(b"GBO1");
    out.push(if decision.matched { 1 } else { 0 });
    out.extend_from_slice(&decision.priority.to_le_bytes());
    let rule = decision.rule_name.as_bytes();
    out.extend_from_slice(&(rule.len() as u16).to_le_bytes());
    out.extend_from_slice(rule);
    let profile = decision.profile.as_bytes();
    out.extend_from_slice(&(profile.len() as u16).to_le_bytes());
    out.extend_from_slice(profile);
    out.extend_from_slice(&(decision.writes.len() as u16).to_le_bytes());
    for (reg, val) in &decision.writes {
        let rb = reg.as_bytes();
        out.extend_from_slice(&(rb.len() as u16).to_le_bytes());
        out.extend_from_slice(rb);
        out.extend_from_slice(&val.to_le_bytes());
    }
    out
}

fn read_u8(raw: &[u8], idx: &mut usize) -> Result<u8, String> {
    if *idx + 1 > raw.len() {
        return Err("buffer underflow on u8".to_string());
    }
    let v = raw[*idx];
    *idx += 1;
    Ok(v)
}

fn read_u16(raw: &[u8], idx: &mut usize) -> Result<u16, String> {
    if *idx + 2 > raw.len() {
        return Err("buffer underflow on u16".to_string());
    }
    let mut b = [0u8; 2];
    b.copy_from_slice(&raw[*idx..*idx + 2]);
    *idx += 2;
    Ok(u16::from_le_bytes(b))
}

fn read_u32(raw: &[u8], idx: &mut usize) -> Result<u32, String> {
    if *idx + 4 > raw.len() {
        return Err("buffer underflow on u32".to_string());
    }
    let mut b = [0u8; 4];
    b.copy_from_slice(&raw[*idx..*idx + 4]);
    *idx += 4;
    Ok(u32::from_le_bytes(b))
}

fn read_i32(raw: &[u8], idx: &mut usize) -> Result<i32, String> {
    if *idx + 4 > raw.len() {
        return Err("buffer underflow on i32".to_string());
    }
    let mut b = [0u8; 4];
    b.copy_from_slice(&raw[*idx..*idx + 4]);
    *idx += 4;
    Ok(i32::from_le_bytes(b))
}

fn read_f64(raw: &[u8], idx: &mut usize) -> Result<f64, String> {
    if *idx + 8 > raw.len() {
        return Err("buffer underflow on f64".to_string());
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&raw[*idx..*idx + 8]);
    *idx += 8;
    Ok(f64::from_le_bytes(b))
}

fn read_str(raw: &[u8], idx: &mut usize) -> Result<String, String> {
    let len = read_u16(raw, idx)? as usize;
    if *idx + len > raw.len() {
        return Err("buffer underflow on string".to_string());
    }
    let s = String::from_utf8(raw[*idx..*idx + len].to_vec())
        .map_err(|e| format!("invalid utf8 string: {e}"))?;
    *idx += len;
    Ok(s)
}

fn load_gbc_bin(path: &PathBuf) -> Result<GbcBinArtifact, String> {
    let raw = fs::read(path).map_err(|e| format!("read gbc bin failed: {e}"))?;
    if raw.len() < 8 {
        return Err("gbc bin too short".to_string());
    }
    if &raw[0..4] != b"GBC2" {
        return Err("invalid gbc bin magic".to_string());
    }
    let mut idx = 4usize;
    let rule_count = read_u32(&raw, &mut idx)? as usize;
    let mut rules = Vec::<GbcBinRule>::new();
    for _ in 0..rule_count {
        let name = read_str(&raw, &mut idx)?;
        let priority = read_i32(&raw, &mut idx)?;
        let pred_count = read_u16(&raw, &mut idx)? as usize;
        let mut predicates = Vec::<GbcBinPredicate>::new();
        for _ in 0..pred_count {
            let field = read_str(&raw, &mut idx)?;
            let op = read_u8(&raw, &mut idx)?;
            let t = read_u8(&raw, &mut idx)?;
            let value = match t {
                0 => PredValue::Num(read_f64(&raw, &mut idx)?),
                1 => PredValue::Bool(read_u8(&raw, &mut idx)? != 0),
                _ => PredValue::Str(read_str(&raw, &mut idx)?),
            };
            predicates.push(GbcBinPredicate { field, op, value });
        }
        let action_kind = read_u8(&raw, &mut idx)?;
        let kv_count = read_u16(&raw, &mut idx)? as usize;
        let mut action_values = HashMap::<String, String>::new();
        for _ in 0..kv_count {
            let k = read_str(&raw, &mut idx)?;
            let v = read_str(&raw, &mut idx)?;
            action_values.insert(k, v);
        }
        rules.push(GbcBinRule {
            name,
            priority,
            predicates,
            action_kind,
            action_values,
        });
    }
    Ok(GbcBinArtifact { rules })
}

fn try_run_gbc_json(
    artifact_path: &PathBuf,
    chip_pack_path: &PathBuf,
    context: &RamanRuntimeContext,
) -> Result<Option<Decision>, String> {
    let raw = fs::read_to_string(artifact_path).map_err(|e| format!("read gbc failed: {e}"))?;
    let artifact =
        serde_json::from_str::<GbcArtifact>(&raw).map_err(|e| format!("parse gbc failed: {e}"))?;
    Ok(run_gbc_json_loaded(&artifact, chip_pack_path, context))
}

fn run_gbc_json_loaded(
    artifact: &GbcArtifact,
    chip_pack_path: &PathBuf,
    context: &RamanRuntimeContext,
) -> Option<Decision> {
    let rhml_path = chip_pack_path.join("mapping.gargantua.rhml");
    let rhml_fallback = chip_pack_path.join("mapping.rhml");
    let rhml_used = if rhml_path.exists() {
        rhml_path
    } else {
        rhml_fallback
    };
    let mappings = parse_rhml_mappings(&rhml_used);
    run_gbc_json_loaded_with_mappings(artifact, &mappings, context)
}

fn run_gbc_json_loaded_with_mappings(
    artifact: &GbcArtifact,
    mappings: &[RhmlRule],
    context: &RamanRuntimeContext,
) -> Option<Decision> {
    let matched_rule = artifact.rules.iter().find(|rule| {
        rule.predicates
            .iter()
            .all(|pred| predicate_matches_json(pred, context))
    });
    let Some(rule) = matched_rule else {
        return None;
    };
    if rule.action.kind != "phy" {
        return None;
    }
    let writes = apply_action_to_writes_with_mappings(&rule.action.values, mappings);
    Some(Decision {
        matched: true,
        rule_name: rule.name.clone(),
        priority: rule.priority,
        profile: rule
            .action
            .values
            .get("name")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        writes,
    })
}

fn try_run_gbc_bin(
    artifact_path: &PathBuf,
    chip_pack_path: &PathBuf,
    context: &RamanRuntimeContext,
) -> Result<Option<Decision>, String> {
    let artifact = load_gbc_bin(artifact_path)?;
    Ok(run_gbc_bin_loaded(&artifact, chip_pack_path, context))
}

fn run_gbc_bin_loaded(
    artifact: &GbcBinArtifact,
    chip_pack_path: &PathBuf,
    context: &RamanRuntimeContext,
) -> Option<Decision> {
    let rhml_path = chip_pack_path.join("mapping.gargantua.rhml");
    let rhml_fallback = chip_pack_path.join("mapping.rhml");
    let rhml_used = if rhml_path.exists() {
        rhml_path
    } else {
        rhml_fallback
    };
    let mappings = parse_rhml_mappings(&rhml_used);
    run_gbc_bin_loaded_with_mappings(artifact, &mappings, context)
}

fn run_gbc_bin_loaded_with_mappings(
    artifact: &GbcBinArtifact,
    mappings: &[RhmlRule],
    context: &RamanRuntimeContext,
) -> Option<Decision> {
    let matched_rule = artifact.rules.iter().find(|rule| {
        rule.predicates
            .iter()
            .all(|pred| predicate_matches_bin(pred, context))
    });
    let Some(rule) = matched_rule else {
        return None;
    };
    if rule.action_kind != 0 {
        return None;
    }
    let writes = apply_action_to_writes_with_mappings(&rule.action_values, mappings);
    Some(Decision {
        matched: true,
        rule_name: rule.name.clone(),
        priority: rule.priority,
        profile: rule
            .action_values
            .get("name")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        writes,
    })
}

fn print_decision(decision: &Decision) {
    println!(
        "chip={} priority={} profile={} downgraded=false writes={}",
        "gargantua-bytecode",
        decision.priority,
        decision.profile,
        decision.writes.len()
    );
    for (reg, val) in &decision.writes {
        println!("write {}=0x{:X}", reg, val);
    }
}

fn main() {
    let mut artifact_path = PathBuf::from("contracts/raman_policy_v1.json");
    let mut fallback_artifact_path = PathBuf::from("contracts/raman_policy_v1.json");
    let mut chip_pack_path = PathBuf::from("chips/espressif/esp32c3_sx1262");
    let mut stream = String::from("portable-cli");
    let mut decision_bin_out: Option<PathBuf> = None;
    let mut bench_iterations: usize = 2000;
    let mut allow_rpl_fallback = true;

    let args = std::env::args().collect::<Vec<_>>();
    let mut idx = 1usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--artifact" if idx + 1 < args.len() => {
                artifact_path = PathBuf::from(&args[idx + 1]);
                idx += 1;
            }
            "--chip-pack" if idx + 1 < args.len() => {
                chip_pack_path = PathBuf::from(&args[idx + 1]);
                idx += 1;
            }
            "--fallback-artifact" if idx + 1 < args.len() => {
                fallback_artifact_path = PathBuf::from(&args[idx + 1]);
                idx += 1;
            }
            "--stream" if idx + 1 < args.len() => {
                stream = args[idx + 1].clone();
                idx += 1;
            }
            "--decision-bin-out" if idx + 1 < args.len() => {
                decision_bin_out = Some(PathBuf::from(&args[idx + 1]));
                idx += 1;
            }
            "--bench-iterations" if idx + 1 < args.len() => {
                if let Ok(v) = args[idx + 1].parse::<usize>() {
                    bench_iterations = v.max(1);
                }
                idx += 1;
            }
            "--no-rpl-fallback" => {
                allow_rpl_fallback = false;
            }
            _ => {}
        }
        idx += 1;
    }

    let context = RamanRuntimeContext {
        minute: 42,
        traffic_class: "truth_critical".to_string(),
        hardware: "sx1262_lab".to_string(),
        scenario: "industrial_shift".to_string(),
        noise_floor_dbm: -108,
        snr_db: 1.5,
        density: 14,
        latency_budget_ms: 250,
        battery_mv: 3630,
        link_margin_db: 2.5,
        confidence: 0.78,
        predicted: "degrading".to_string(),
        drift_db: 0.8,
        renegotiation_needed: true,
        rx_pdr: 0.62,
        error_rate: 0.16,
        path_stability: 0.51,
        memory_confidence: 0.48,
        worst_hop_reliability: 0.44,
    };

    let is_gbc_json = artifact_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".gbc.json"));
    let is_gbc_bin = artifact_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".gbc.bin"));

    if is_gbc_json || is_gbc_bin {
        let decision_result = if is_gbc_bin {
            try_run_gbc_bin(&artifact_path, &chip_pack_path, &context)
        } else {
            try_run_gbc_json(&artifact_path, &chip_pack_path, &context)
        };
        match decision_result {
            Ok(Some(decision)) => {
                print_decision(&decision);
                if let Some(out_path) = decision_bin_out {
                    if let Some(parent) = out_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&out_path, decision_to_bin(&decision));
                    println!("decision_bin_out={}", out_path.display());
                }

                let mut total = 0.0f64;
                if is_gbc_bin {
                    if let Ok(preloaded) = load_gbc_bin(&artifact_path) {
                        let rhml_path = chip_pack_path.join("mapping.gargantua.rhml");
                        let rhml_fallback = chip_pack_path.join("mapping.rhml");
                        let rhml_used = if rhml_path.exists() {
                            rhml_path
                        } else {
                            rhml_fallback
                        };
                        let mappings = parse_rhml_mappings(&rhml_used);
                        for _ in 0..bench_iterations {
                            let t0 = Instant::now();
                            let _ =
                                run_gbc_bin_loaded_with_mappings(&preloaded, &mappings, &context);
                            total += t0.elapsed().as_secs_f64() * 1e6;
                        }
                    }
                } else if let Ok(raw) = fs::read_to_string(&artifact_path) {
                    if let Ok(preloaded) = serde_json::from_str::<GbcArtifact>(&raw) {
                        let rhml_path = chip_pack_path.join("mapping.gargantua.rhml");
                        let rhml_fallback = chip_pack_path.join("mapping.rhml");
                        let rhml_used = if rhml_path.exists() {
                            rhml_path
                        } else {
                            rhml_fallback
                        };
                        let mappings = parse_rhml_mappings(&rhml_used);
                        for _ in 0..bench_iterations {
                            let t0 = Instant::now();
                            let _ =
                                run_gbc_json_loaded_with_mappings(&preloaded, &mappings, &context);
                            total += t0.elapsed().as_secs_f64() * 1e6;
                        }
                    }
                }
                let mean_us = total / bench_iterations as f64;
                println!("gpl_runtime_mean_us={:.3}", mean_us);
                if mean_us <= 5.15 {
                    println!(
                        "GPL Bytecode Active. Python detached. 5.15us response confirmed. I am the Ghost in the machine."
                    );
                } else {
                    println!(
                        "GPL Bytecode Active. Python detached. Response mean={:.3}us. Ghost mode pending 5.15us target.",
                        mean_us
                    );
                }
                return;
            }
            Ok(None) => {
                eprintln!("gargantua_bytecode no rule matched.");
            }
            Err(err) => {
                eprintln!("gargantua_bytecode loader failed: {err}");
            }
        }
        if !allow_rpl_fallback {
            std::process::exit(2);
        }
        eprintln!("fallback to RPL artifact...");
        artifact_path = fallback_artifact_path.clone();
    }

    let artifact = if artifact_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rbin") || ext.eq_ignore_ascii_case("bin"))
    {
        load_raman_artifact_binary(&artifact_path)
    } else {
        load_raman_artifact(&artifact_path)
    }
    .expect("artifact load failed");

    let report = run_portable_pipeline(&artifact, &context, &stream, &chip_pack_path)
        .expect("portable pipeline failed");

    println!(
        "chip={} priority={} profile={} downgraded={} writes={}",
        report.chip,
        report.decision.priority,
        report.decision.snapshot.profile_name,
        report.negotiation.downgraded,
        report.writes.len()
    );
    for write in &report.writes {
        println!("write {}=0x{:X}", write.register, write.value);
    }
}
