#![no_std]

use heapless::Vec;
#[cfg(test)]
extern crate std;

pub type SymbolId = u32;

pub const fn symbol_id_const(bytes: &[u8]) -> SymbolId {
    let mut hash = 0x811C_9DC5u32;
    let mut idx = 0usize;
    while idx < bytes.len() {
        hash ^= bytes[idx] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        idx += 1;
    }
    hash
}

pub fn symbol_id(value: &str) -> SymbolId {
    symbol_id_const(value.as_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldId {
    Minute,
    SnrX10,
    Density,
    KnowledgeMatchQ,
    NoiseQ,
    FluxStableQ,
    BatteryMv,
    BatteryPct,
    JammingDetected,
    TrafficClass,
    Hardware,
    Scenario,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Predicate {
    pub field: FieldId,
    pub op: CompareOp,
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreAction {
    pub profile_id: u16,
    pub transport_id: u16,
    pub ris_id: u16,
    pub hold_ticks: u8,
    pub cooldown_ticks: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreRule<const MAX_PREDICATES: usize> {
    pub priority: i16,
    pub order: u16,
    pub predicates: Vec<Predicate, MAX_PREDICATES>,
    pub action: CoreAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreProgram<const MAX_RULES: usize, const MAX_PREDICATES: usize> {
    pub rules: Vec<CoreRule<MAX_PREDICATES>, MAX_RULES>,
}

impl<const MAX_RULES: usize, const MAX_PREDICATES: usize> Default
    for CoreProgram<MAX_RULES, MAX_PREDICATES>
{
    fn default() -> Self {
        Self { rules: Vec::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreContext {
    pub minute: i32,
    pub snr_db_x10: i16,
    pub density: i16,
    pub knowledge_match_q: u16,
    pub noise_q: u16,
    pub flux_stable_q: u16,
    pub battery_mv: i32,
    pub battery_pct: i16,
    pub jamming_detected: bool,
    pub traffic_id: SymbolId,
    pub hardware_id: SymbolId,
    pub scenario_id: SymbolId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreDecision {
    pub selected_priority: i16,
    pub selected_order: u16,
    pub action: CoreAction,
    pub temporal_reused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoreExecutorState {
    pub active_action: Option<CoreAction>,
    pub active_priority: i16,
    pub active_order: u16,
    pub remaining_hold_ticks: u8,
    pub remaining_cooldown_ticks: u8,
}

pub fn execute<const MAX_RULES: usize, const MAX_PREDICATES: usize>(
    program: &CoreProgram<MAX_RULES, MAX_PREDICATES>,
    context: &CoreContext,
    state: &mut CoreExecutorState,
) -> Option<CoreDecision> {
    if state.remaining_hold_ticks > 0 {
        state.remaining_hold_ticks -= 1;
        if let Some(action) = state.active_action {
            return Some(CoreDecision {
                selected_priority: state.active_priority,
                selected_order: state.active_order,
                action,
                temporal_reused: true,
            });
        }
    }

    if state.remaining_cooldown_ticks > 0 {
        state.remaining_cooldown_ticks -= 1;
        if let Some(action) = state.active_action {
            return Some(CoreDecision {
                selected_priority: state.active_priority,
                selected_order: state.active_order,
                action,
                temporal_reused: true,
            });
        }
    }

    let mut best: Option<&CoreRule<MAX_PREDICATES>> = None;
    for rule in program.rules.iter() {
        if !rule_matches(rule, context) {
            continue;
        }
        if let Some(current) = best {
            if rule.priority > current.priority
                || (rule.priority == current.priority && rule.order < current.order)
            {
                best = Some(rule);
            }
        } else {
            best = Some(rule);
        }
    }

    let Some(selected) = best else {
        // Keep temporal counters authoritative; once they are exhausted and no
        // rule matches, clear the stale action snapshot.
        state.active_action = None;
        state.active_priority = 0;
        state.active_order = 0;
        state.remaining_hold_ticks = 0;
        state.remaining_cooldown_ticks = 0;
        return None;
    };
    state.active_priority = selected.priority;
    state.active_order = selected.order;
    state.active_action = Some(selected.action);
    state.remaining_hold_ticks = selected.action.hold_ticks;
    state.remaining_cooldown_ticks = selected.action.cooldown_ticks;
    Some(CoreDecision {
        selected_priority: selected.priority,
        selected_order: selected.order,
        action: selected.action,
        temporal_reused: false,
    })
}

fn rule_matches<const MAX_PREDICATES: usize>(
    rule: &CoreRule<MAX_PREDICATES>,
    context: &CoreContext,
) -> bool {
    rule.predicates
        .iter()
        .all(|predicate| predicate_matches(predicate, context))
}

fn predicate_matches(predicate: &Predicate, context: &CoreContext) -> bool {
    let left = match predicate.field {
        FieldId::Minute => context.minute,
        FieldId::SnrX10 => context.snr_db_x10 as i32,
        FieldId::Density => context.density as i32,
        FieldId::KnowledgeMatchQ => context.knowledge_match_q as i32,
        FieldId::NoiseQ => context.noise_q as i32,
        FieldId::FluxStableQ => context.flux_stable_q as i32,
        FieldId::BatteryMv => context.battery_mv,
        FieldId::BatteryPct => context.battery_pct as i32,
        FieldId::JammingDetected => {
            if context.jamming_detected {
                1
            } else {
                0
            }
        }
        FieldId::TrafficClass => context.traffic_id as i32,
        FieldId::Hardware => context.hardware_id as i32,
        FieldId::Scenario => context.scenario_id as i32,
    };
    match predicate.op {
        CompareOp::Eq => left == predicate.value,
        CompareOp::Ne => left != predicate.value,
        CompareOp::Lt => left < predicate.value,
        CompareOp::Le => left <= predicate.value,
        CompareOp::Gt => left > predicate.value,
        CompareOp::Ge => left >= predicate.value,
    }
}

pub const SIGNAL_AGNOSTIC_STATUS: &str = "Signal-Agnostic system: RAMAN/GPLang/COOPER";
pub const FULL_SOVEREIGNTY_STATUS: &str = "Full Sovereignty Reached";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalMedium {
    Radio,
    Satellite,
    Optical,
    Wireline,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighRateLinkProfile {
    pub medium: SignalMedium,
    pub sample_rate_hz: u32,
    pub frame_bytes: u16,
    pub burst_window_us: u32,
    pub jitter_budget_us: u32,
    pub requires_ack: bool,
}

pub const SATELLITE_LIKE_PROFILE: HighRateLinkProfile = HighRateLinkProfile {
    medium: SignalMedium::Satellite,
    sample_rate_hz: 2_000_000,
    frame_bytes: 256,
    burst_window_us: 120,
    jitter_budget_us: 40,
    requires_ack: true,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighRateFrame {
    pub seq: u32,
    pub timestamp_us: u32,
    pub confidence_q: u16,
    pub snr_db_x10: i16,
    pub density: i16,
    pub battery_mv: i32,
    pub battery_pct: i16,
    pub jamming_detected: bool,
}

pub trait SignalAgnosticStream {
    fn link_profile(&self) -> HighRateLinkProfile;
    fn read_frame(&mut self) -> Option<HighRateFrame>;
}

pub fn frame_to_core_context(
    frame: HighRateFrame,
    minute: i32,
    traffic_id: SymbolId,
    hardware_id: SymbolId,
    scenario_id: SymbolId,
) -> CoreContext {
    CoreContext {
        minute,
        snr_db_x10: frame.snr_db_x10,
        density: frame.density,
        knowledge_match_q: 0,
        noise_q: 0,
        flux_stable_q: 1000,
        battery_mv: frame.battery_mv,
        battery_pct: frame.battery_pct,
        jamming_detected: frame.jamming_detected,
        traffic_id,
        hardware_id,
        scenario_id,
    }
}

pub fn stream_tick<const MAX_RULES: usize, const MAX_PREDICATES: usize, S: SignalAgnosticStream>(
    stream: &mut S,
    minute: i32,
    traffic_id: SymbolId,
    hardware_id: SymbolId,
    scenario_id: SymbolId,
    program: &CoreProgram<MAX_RULES, MAX_PREDICATES>,
    state: &mut CoreExecutorState,
) -> Option<CoreDecision> {
    let frame = stream.read_frame()?;
    let context = frame_to_core_context(frame, minute, traffic_id, hardware_id, scenario_id);
    execute(program, &context, state)
}

#[cfg(test)]
mod tests {
    use super::{
        execute, stream_tick, symbol_id, CompareOp, CoreAction, CoreContext, CoreExecutorState,
        CoreProgram, CoreRule, FieldId, HighRateFrame, Predicate, SignalAgnosticStream,
        SignalMedium, SATELLITE_LIKE_PROFILE,
    };
    use core::str::FromStr;
    use heapless::Vec;
    use std::path::PathBuf;
    use std::process::Command;
    use std::string::String;

    #[test]
    fn selects_highest_priority_rule_and_reuses_hold_window() {
        let mut program = CoreProgram::<8, 4>::default();

        let mut resilient_predicates = Vec::<Predicate, 4>::new();
        let _ = resilient_predicates.push(Predicate {
            field: FieldId::SnrX10,
            op: CompareOp::Lt,
            value: 30,
        });
        let _ = program.rules.push(CoreRule {
            priority: 300,
            order: 1,
            predicates: resilient_predicates,
            action: CoreAction {
                profile_id: 2,
                transport_id: 11,
                ris_id: 0,
                hold_ticks: 2,
                cooldown_ticks: 1,
            },
        });

        let mut fastlane_predicates = Vec::<Predicate, 4>::new();
        let _ = fastlane_predicates.push(Predicate {
            field: FieldId::TrafficClass,
            op: CompareOp::Eq,
            value: symbol_id("voice") as i32,
        });
        let _ = program.rules.push(CoreRule {
            priority: 100,
            order: 2,
            predicates: fastlane_predicates,
            action: CoreAction {
                profile_id: 1,
                transport_id: 7,
                ris_id: 0,
                hold_ticks: 0,
                cooldown_ticks: 0,
            },
        });

        let context = CoreContext {
            minute: 12,
            snr_db_x10: 10,
            density: 7,
            knowledge_match_q: 0,
            noise_q: 0,
            flux_stable_q: 1000,
            battery_mv: 3650,
            battery_pct: 54,
            jamming_detected: false,
            traffic_id: symbol_id("voice"),
            hardware_id: symbol_id("esp32c3_sx1262"),
            scenario_id: symbol_id("industrial_shift"),
        };
        let mut state = CoreExecutorState::default();

        let decision_1 = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision_1.selected_priority, 300);
        assert_eq!(decision_1.action.profile_id, 2);
        assert!(!decision_1.temporal_reused);

        let decision_2 = execute(&program, &context, &mut state).expect("decision expected");
        assert!(decision_2.temporal_reused);
        assert_eq!(decision_2.action.profile_id, 2);
    }

    #[test]
    fn clears_stale_action_when_no_rule_matches_after_temporal_windows() {
        let mut program = CoreProgram::<8, 4>::default();
        let mut predicates = Vec::<Predicate, 4>::new();
        let _ = predicates.push(Predicate {
            field: FieldId::BatteryMv,
            op: CompareOp::Lt,
            value: 3300,
        });
        let _ = program.rules.push(CoreRule {
            priority: 100,
            order: 1,
            predicates,
            action: CoreAction {
                profile_id: 7,
                transport_id: 1,
                ris_id: 0,
                hold_ticks: 1,
                cooldown_ticks: 1,
            },
        });

        let mut state = CoreExecutorState::default();
        let low_battery_ctx = CoreContext {
            minute: 1,
            snr_db_x10: 0,
            density: 0,
            knowledge_match_q: 0,
            noise_q: 0,
            flux_stable_q: 1000,
            battery_mv: 3200,
            battery_pct: 16,
            jamming_detected: false,
            traffic_id: symbol_id("bulk"),
            hardware_id: symbol_id("esp32c3_sx1262"),
            scenario_id: symbol_id("industrial_shift"),
        };
        let healthy_battery_ctx = CoreContext {
            battery_mv: 3800,
            ..low_battery_ctx
        };

        let first = execute(&program, &low_battery_ctx, &mut state).expect("decision expected");
        assert_eq!(first.action.profile_id, 7);
        assert!(!first.temporal_reused);

        let hold = execute(&program, &healthy_battery_ctx, &mut state).expect("decision expected");
        assert!(hold.temporal_reused);
        let cooldown =
            execute(&program, &healthy_battery_ctx, &mut state).expect("decision expected");
        assert!(cooldown.temporal_reused);

        let none = execute(&program, &healthy_battery_ctx, &mut state);
        assert!(none.is_none());
        assert_eq!(state.active_action, None);
        assert_eq!(state.remaining_hold_ticks, 0);
        assert_eq!(state.remaining_cooldown_ticks, 0);
    }

    #[test]
    fn tie_breaks_same_priority_by_lower_order() {
        let mut program = CoreProgram::<8, 4>::default();

        let mut p1 = Vec::<Predicate, 4>::new();
        let _ = p1.push(Predicate {
            field: FieldId::TrafficClass,
            op: CompareOp::Eq,
            value: symbol_id("voice") as i32,
        });
        let _ = program.rules.push(CoreRule {
            priority: 200,
            order: 10,
            predicates: p1,
            action: CoreAction {
                profile_id: 1,
                transport_id: 1,
                ris_id: 0,
                hold_ticks: 0,
                cooldown_ticks: 0,
            },
        });

        let mut p2 = Vec::<Predicate, 4>::new();
        let _ = p2.push(Predicate {
            field: FieldId::TrafficClass,
            op: CompareOp::Eq,
            value: symbol_id("voice") as i32,
        });
        let _ = program.rules.push(CoreRule {
            priority: 200,
            order: 2,
            predicates: p2,
            action: CoreAction {
                profile_id: 9,
                transport_id: 9,
                ris_id: 0,
                hold_ticks: 0,
                cooldown_ticks: 0,
            },
        });

        let context = CoreContext {
            minute: 0,
            snr_db_x10: 0,
            density: 0,
            knowledge_match_q: 0,
            noise_q: 0,
            flux_stable_q: 1000,
            battery_mv: 3700,
            battery_pct: 60,
            jamming_detected: false,
            traffic_id: symbol_id("voice"),
            hardware_id: symbol_id("esp32c3_sx1262"),
            scenario_id: symbol_id("industrial_shift"),
        };
        let mut state = CoreExecutorState::default();
        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 200);
        assert_eq!(decision.selected_order, 2);
        assert_eq!(decision.action.profile_id, 9);
    }

    #[test]
    fn knowledge_match_predicate_gates_high_priority_rule() {
        let mut program = CoreProgram::<8, 4>::default();

        let mut resonance_predicates = Vec::<Predicate, 4>::new();
        let _ = resonance_predicates.push(Predicate {
            field: FieldId::KnowledgeMatchQ,
            op: CompareOp::Ge,
            value: 600,
        });
        let _ = program.rules.push(CoreRule {
            priority: 300,
            order: 1,
            predicates: resonance_predicates,
            action: CoreAction {
                profile_id: 42,
                transport_id: 7,
                ris_id: 0,
                hold_ticks: 0,
                cooldown_ticks: 0,
            },
        });

        let mut fallback_predicates = Vec::<Predicate, 4>::new();
        let _ = fallback_predicates.push(Predicate {
            field: FieldId::TrafficClass,
            op: CompareOp::Eq,
            value: symbol_id("voice") as i32,
        });
        let _ = program.rules.push(CoreRule {
            priority: 100,
            order: 2,
            predicates: fallback_predicates,
            action: CoreAction {
                profile_id: 7,
                transport_id: 1,
                ris_id: 0,
                hold_ticks: 0,
                cooldown_ticks: 0,
            },
        });

        let mut state = CoreExecutorState::default();
        let mut context = CoreContext {
            minute: 0,
            snr_db_x10: 0,
            density: 0,
            knowledge_match_q: 650,
            noise_q: 0,
            flux_stable_q: 1000,
            battery_mv: 3700,
            battery_pct: 60,
            jamming_detected: false,
            traffic_id: symbol_id("voice"),
            hardware_id: symbol_id("esp32c3_sx1262"),
            scenario_id: symbol_id("industrial_shift"),
        };

        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 300);
        assert_eq!(decision.action.profile_id, 42);

        state = CoreExecutorState::default();
        context.knowledge_match_q = 300;
        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 100);
        assert_eq!(decision.action.profile_id, 7);
    }

    #[test]
    fn ril_autonomous_sentinel_compiles_and_executes_in_core_end_to_end() {
        let compiled_rpl = compile_autonomous_sentinel_ril_to_rpl();
        assert!(compiled_rpl.contains("radio(state=off, mode=silent, power=off)"));
        assert!(compiled_rpl.contains("phy(name=ril_immune_long_range"));
        assert!(compiled_rpl.contains("phy(name=ril_telemetry_broadcast_low"));

        let program = build_core_program_from_compiled_rpl(&compiled_rpl);
        assert!(!program.rules.is_empty());

        let mut state = CoreExecutorState::default();
        let mut context = sentinel_context();
        context.battery_pct = 15;
        context.jamming_detected = false;
        context.snr_db_x10 = 0;
        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 500);

        state = CoreExecutorState::default();
        context.battery_pct = 85;
        context.jamming_detected = true;
        context.snr_db_x10 = 20;
        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 500);

        state = CoreExecutorState::default();
        context.battery_pct = 80;
        context.jamming_detected = false;
        context.snr_db_x10 = -120; // -12 dB
        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 300);

        state = CoreExecutorState::default();
        context.battery_pct = 80;
        context.jamming_detected = false;
        context.snr_db_x10 = -20; // -2 dB
        let decision = execute(&program, &context, &mut state).expect("decision expected");
        assert_eq!(decision.selected_priority, 100);
    }

    fn sentinel_context() -> CoreContext {
        CoreContext {
            minute: 0,
            snr_db_x10: 0,
            density: 42,
            knowledge_match_q: 0,
            noise_q: 0,
            flux_stable_q: 1000,
            battery_mv: 3600,
            battery_pct: 60,
            jamming_detected: false,
            traffic_id: symbol_id("truth_critical"),
            hardware_id: symbol_id("generic"),
            scenario_id: symbol_id("default"),
        }
    }

    fn compile_autonomous_sentinel_ril_to_rpl() -> String {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir.parent().expect("repo root");
        let script = "from pathlib import Path; from sim.radnet_sim.ril import compile_ril_to_rpl; p=Path('policies/ril/autonomous_sentinel.ril'); print(compile_ril_to_rpl(p.read_text(encoding='utf-8')))";
        let output = Command::new("python")
            .arg("-c")
            .arg(script)
            .current_dir(repo_root)
            .output()
            .expect("python compile_ril_to_rpl should run");
        assert!(
            output.status.success(),
            "compile_ril_to_rpl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("compiled RPL utf-8")
    }

    fn build_core_program_from_compiled_rpl(rpl_source: &str) -> CoreProgram<64, 8> {
        let mut program = CoreProgram::<64, 8>::default();
        for (order, raw) in rpl_source.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || !line.starts_with("priority ") {
                continue;
            }
            let (priority, predicate_text, action_text) =
                parse_rpl_line(line).expect("valid compiled RPL line");
            let mut predicates = Vec::<Predicate, 8>::new();
            for token in predicate_text.split(" and ") {
                if let Some(predicate) = parse_predicate(token.trim()) {
                    let _ = predicates.push(predicate);
                }
            }
            let _ = program.rules.push(CoreRule {
                priority,
                order: order as u16,
                predicates,
                action: action_from_text(priority, action_text),
            });
        }
        program
    }

    fn parse_rpl_line(line: &str) -> Option<(i16, &str, &str)> {
        let after_priority = line.strip_prefix("priority ")?;
        let (priority_text, after_priority_value) = after_priority.split_once(" when ")?;
        let (predicate_text, action_text) = after_priority_value.split_once(" -> ")?;
        let priority = i16::from_str(priority_text.trim()).ok()?;
        Some((priority, predicate_text.trim(), action_text.trim()))
    }

    fn parse_predicate(token: &str) -> Option<Predicate> {
        let operators = [
            (">=", CompareOp::Ge),
            ("<=", CompareOp::Le),
            ("==", CompareOp::Eq),
            (">", CompareOp::Gt),
            ("<", CompareOp::Lt),
        ];
        for (marker, op) in operators {
            if let Some((left, right)) = token.split_once(marker) {
                let field = match left.trim() {
                    "minute" => FieldId::Minute,
                    "snr" => FieldId::SnrX10,
                    "density" => FieldId::Density,
                    "knowledge_match" => FieldId::KnowledgeMatchQ,
                    "noise_q" => FieldId::NoiseQ,
                    "flux_stable_q" => FieldId::FluxStableQ,
                    "battery_mv" => FieldId::BatteryMv,
                    "battery_pct" => FieldId::BatteryPct,
                    "jamming_detected" => FieldId::JammingDetected,
                    "traffic" => FieldId::TrafficClass,
                    "hardware" => FieldId::Hardware,
                    "scenario" => FieldId::Scenario,
                    _ => return None,
                };
                let value = parse_value(field, right.trim())?;
                return Some(Predicate { field, op, value });
            }
        }
        None
    }

    fn parse_value(field: FieldId, raw: &str) -> Option<i32> {
        let value = raw.trim().trim_matches('"').trim_matches('\'');
        match field {
            FieldId::SnrX10 => {
                let db = f32::from_str(value).ok()?;
                Some((db * 10.0).round() as i32)
            }
            FieldId::TrafficClass | FieldId::Hardware | FieldId::Scenario => {
                Some(symbol_id(value) as i32)
            }
            FieldId::JammingDetected => Some((value.eq_ignore_ascii_case("true")) as i32),
            _ => i32::from_str(value).ok(),
        }
    }

    fn action_from_text(priority: i16, action_text: &str) -> CoreAction {
        let kind = action_text.split('(').next().unwrap_or("").trim();
        let profile_id = if kind == "radio" {
            90
        } else {
            priority.max(0) as u16
        };
        let transport_id = if kind == "transport" { 3 } else { 0 };
        let ris_id = if kind == "ris" { 4 } else { 0 };
        let hold_ticks = extract_ticks(action_text, "hold");
        let cooldown_ticks = extract_ticks(action_text, "cooldown");
        CoreAction {
            profile_id,
            transport_id,
            ris_id,
            hold_ticks,
            cooldown_ticks,
        }
    }

    fn extract_ticks(action_text: &str, expected_kind: &str) -> u8 {
        if !action_text.starts_with(expected_kind) {
            return 0;
        }
        let payload = action_text
            .split_once('(')
            .and_then(|(_, tail)| tail.strip_suffix(')'))
            .unwrap_or("");
        for pair in payload.split(',') {
            if let Some((key, value)) = pair.split_once('=') {
                if key.trim() == "ticks" {
                    return u8::from_str(value.trim()).unwrap_or(0);
                }
            }
        }
        0
    }
}
