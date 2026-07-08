use heapless::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeterministicMode {
    Nominal,
    Degraded,
    Survival,
    Silent,
    BeaconOnly,
    StoreAndForward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultSymptom {
    AckTimeoutBurst,
    CrcSpike,
    ThermalRunaway,
    QueueOverrun,
    ClockDriftSpike,
    SignatureMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    Link,
    Integrity,
    Thermal,
    Scheduling,
    Timing,
    Security,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    RenegotiateProfile,
    SwitchMode(DeterministicMode),
    ResetRadio,
    KeyResync,
    DrainQueue,
    RebootSubsystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultEvent {
    pub symptom: FaultSymptom,
    pub severity: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdirReport {
    pub class: FaultClass,
    pub actions: Vec<RecoveryAction, 6>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FdirEngine;

impl FdirEngine {
    pub fn isolate_and_plan(&self, event: FaultEvent) -> FdirReport {
        let class = isolate_fault(event.symptom);
        let mut actions = Vec::<RecoveryAction, 6>::new();
        match class {
            FaultClass::Link => {
                let _ = actions.push(RecoveryAction::RenegotiateProfile);
                let _ = actions.push(RecoveryAction::SwitchMode(DeterministicMode::Survival));
                let _ = actions.push(RecoveryAction::ResetRadio);
            }
            FaultClass::Integrity => {
                let _ = actions.push(RecoveryAction::SwitchMode(
                    DeterministicMode::StoreAndForward,
                ));
                let _ = actions.push(RecoveryAction::ResetRadio);
            }
            FaultClass::Thermal => {
                let _ = actions.push(RecoveryAction::SwitchMode(DeterministicMode::Silent));
                let _ = actions.push(RecoveryAction::DrainQueue);
            }
            FaultClass::Scheduling => {
                let _ = actions.push(RecoveryAction::DrainQueue);
                let _ = actions.push(RecoveryAction::SwitchMode(DeterministicMode::BeaconOnly));
            }
            FaultClass::Timing => {
                let _ = actions.push(RecoveryAction::SwitchMode(DeterministicMode::Degraded));
                let _ = actions.push(RecoveryAction::ResetRadio);
            }
            FaultClass::Security => {
                let _ = actions.push(RecoveryAction::KeyResync);
                let _ = actions.push(RecoveryAction::SwitchMode(DeterministicMode::Silent));
            }
        }
        if event.severity >= 9 {
            let _ = actions.push(RecoveryAction::RebootSubsystem);
        }
        FdirReport { class, actions }
    }
}

fn isolate_fault(symptom: FaultSymptom) -> FaultClass {
    match symptom {
        FaultSymptom::AckTimeoutBurst => FaultClass::Link,
        FaultSymptom::CrcSpike => FaultClass::Integrity,
        FaultSymptom::ThermalRunaway => FaultClass::Thermal,
        FaultSymptom::QueueOverrun => FaultClass::Scheduling,
        FaultSymptom::ClockDriftSpike => FaultClass::Timing,
        FaultSymptom::SignatureMismatch => FaultClass::Security,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuntimeTelemetry {
    pub tx_power_dbm: i8,
    pub duty_cycle_permille: u16,
    pub latency_ms: u16,
    pub thermal_c: i16,
    pub queue_depth: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyViolation {
    TxPowerLimit,
    DutyCycleLimit,
    LatencyLimit,
    ThermalLimit,
    QueueLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyCheck {
    pub pass: bool,
    pub violations: Vec<SafetyViolation, 8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyInvariants {
    pub max_tx_power_dbm: i8,
    pub max_duty_cycle_permille: u16,
    pub max_latency_ms: u16,
    pub max_thermal_c: i16,
    pub max_queue_depth: u16,
}

impl SafetyInvariants {
    pub fn evaluate(&self, telemetry: RuntimeTelemetry) -> SafetyCheck {
        let mut violations = Vec::<SafetyViolation, 8>::new();
        if telemetry.tx_power_dbm > self.max_tx_power_dbm {
            let _ = violations.push(SafetyViolation::TxPowerLimit);
        }
        if telemetry.duty_cycle_permille > self.max_duty_cycle_permille {
            let _ = violations.push(SafetyViolation::DutyCycleLimit);
        }
        if telemetry.latency_ms > self.max_latency_ms {
            let _ = violations.push(SafetyViolation::LatencyLimit);
        }
        if telemetry.thermal_c > self.max_thermal_c {
            let _ = violations.push(SafetyViolation::ThermalLimit);
        }
        if telemetry.queue_depth > self.max_queue_depth {
            let _ = violations.push(SafetyViolation::QueueLimit);
        }
        SafetyCheck {
            pass: violations.is_empty(),
            violations,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeDecisionDigest {
    pub profile_id: u16,
    pub priority_id: u16,
    pub mode: DeterministicMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedundantRuntimeVerdict {
    pub accepted: RuntimeDecisionDigest,
    pub disagreement: bool,
    pub rollback_used: bool,
}

pub fn redundant_vote(
    primary: RuntimeDecisionDigest,
    shadow: RuntimeDecisionDigest,
    checkpoint: RuntimeDecisionDigest,
) -> RedundantRuntimeVerdict {
    if primary == shadow {
        return RedundantRuntimeVerdict {
            accepted: primary,
            disagreement: false,
            rollback_used: false,
        };
    }
    RedundantRuntimeVerdict {
        accepted: checkpoint,
        disagreement: true,
        rollback_used: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthInput {
    pub ack_ratio: f32,
    pub error_rate: f32,
    pub queue_depth_norm: f32,
    pub thermal_margin_norm: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthGovernor {
    pub degrade_threshold: i16,
    pub recover_threshold: i16,
    pub score: i16,
    pub mode: DeterministicMode,
}

impl Default for HealthGovernor {
    fn default() -> Self {
        Self {
            degrade_threshold: 55,
            recover_threshold: 75,
            score: 100,
            mode: DeterministicMode::Nominal,
        }
    }
}

impl HealthGovernor {
    pub fn update(&mut self, input: HealthInput) -> DeterministicMode {
        let ack = (input.ack_ratio.clamp(0.0, 1.0) * 100.0) as i16;
        let err_penalty = (input.error_rate.clamp(0.0, 1.0) * 50.0) as i16;
        let queue_penalty = (input.queue_depth_norm.clamp(0.0, 1.0) * 30.0) as i16;
        let thermal_bonus = (input.thermal_margin_norm.clamp(0.0, 1.0) * 20.0) as i16;
        self.score = (ack - err_penalty - queue_penalty + thermal_bonus).clamp(0, 100);

        self.mode = if self.score < 25 {
            DeterministicMode::Silent
        } else if self.score < self.degrade_threshold {
            DeterministicMode::Survival
        } else if self.mode != DeterministicMode::Nominal && self.score >= self.recover_threshold {
            DeterministicMode::Nominal
        } else if self.score < self.recover_threshold {
            DeterministicMode::Degraded
        } else {
            DeterministicMode::Nominal
        };
        self.mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalContracts {
    pub detect_us_max: u32,
    pub decision_us_max: u32,
    pub apply_us_max: u32,
    pub recover_us_max: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalSample {
    pub detect_us: u32,
    pub decision_us: u32,
    pub apply_us: u32,
    pub recover_us: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalReport {
    pub pass: bool,
    pub detect_ok: bool,
    pub decision_ok: bool,
    pub apply_ok: bool,
    pub recover_ok: bool,
}

impl TemporalContracts {
    pub fn evaluate(&self, sample: TemporalSample) -> TemporalReport {
        let detect_ok = sample.detect_us <= self.detect_us_max;
        let decision_ok = sample.decision_us <= self.decision_us_max;
        let apply_ok = sample.apply_us <= self.apply_us_max;
        let recover_ok = sample.recover_us <= self.recover_us_max;
        TemporalReport {
            pass: detect_ok && decision_ok && apply_ok && recover_ok,
            detect_ok,
            decision_ok,
            apply_ok,
            recover_ok,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeuSlotState {
    pub crc_ok: bool,
    pub ecc_corrected_bits: u16,
    pub bitflip_count: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeuGuardReport {
    pub pass: bool,
    pub scrub_required: bool,
}

pub fn evaluate_seu(
    slot_a: SeuSlotState,
    slot_b: SeuSlotState,
    scrub_period_ticks: u32,
    tick: u32,
) -> SeuGuardReport {
    let scrub_required = scrub_period_ticks > 0 && tick % scrub_period_ticks == 0;
    let pass = (slot_a.crc_ok || slot_b.crc_ok)
        && slot_a.bitflip_count < 64
        && slot_b.bitflip_count < 64
        && (slot_a.ecc_corrected_bits + slot_b.ecc_corrected_bits) < 256;
    SeuGuardReport {
        pass,
        scrub_required,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureMissionInput {
    pub signature_ok: bool,
    pub boot_chain_ok: bool,
    pub key_epoch: u32,
    pub min_allowed_epoch: u32,
    pub key_revoked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureMissionReport {
    pub pass: bool,
    pub reason: &'static str,
}

pub fn verify_secure_chain(input: SecureMissionInput) -> SecureMissionReport {
    if !input.boot_chain_ok {
        return SecureMissionReport {
            pass: false,
            reason: "boot_chain_failed",
        };
    }
    if input.key_revoked {
        return SecureMissionReport {
            pass: false,
            reason: "key_revoked",
        };
    }
    if input.key_epoch < input.min_allowed_epoch {
        return SecureMissionReport {
            pass: false,
            reason: "stale_key_epoch",
        };
    }
    if !input.signature_ok {
        return SecureMissionReport {
            pass: false,
            reason: "signature_invalid",
        };
    }
    SecureMissionReport {
        pass: true,
        reason: "ok",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwinHardwareMetrics {
    pub pdr: f32,
    pub latency_ms: f32,
    pub recover_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParityThresholds {
    pub pdr_delta_max: f32,
    pub latency_delta_max_ms: f32,
    pub recover_delta_max_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParityGateReport {
    pub pass: bool,
    pub pdr_delta: f32,
    pub latency_delta_ms: f32,
    pub recover_delta_ms: f32,
}

pub fn parity_gate(
    twin: TwinHardwareMetrics,
    hardware: TwinHardwareMetrics,
    thresholds: ParityThresholds,
) -> ParityGateReport {
    let pdr_delta = (twin.pdr - hardware.pdr).abs();
    let latency_delta_ms = (twin.latency_ms - hardware.latency_ms).abs();
    let recover_delta_ms = (twin.recover_ms - hardware.recover_ms).abs();
    let pass = pdr_delta <= thresholds.pdr_delta_max
        && latency_delta_ms <= thresholds.latency_delta_max_ms
        && recover_delta_ms <= thresholds.recover_delta_max_ms;
    ParityGateReport {
        pass,
        pdr_delta,
        latency_delta_ms,
        recover_delta_ms,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidualRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScenarioCertificationRow {
    pub scenario_id: u32,
    pub safety_pass: bool,
    pub temporal_pass: bool,
    pub security_pass: bool,
    pub parity_pass: bool,
    pub residual_risk: ResidualRisk,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioCertificationReport {
    pub pass: bool,
    pub passed_rows: usize,
    pub failed_rows: usize,
    pub max_residual_risk: ResidualRisk,
}

pub fn certify_scenarios<const N: usize>(
    rows: &Vec<ScenarioCertificationRow, N>,
) -> ScenarioCertificationReport {
    let mut passed_rows = 0usize;
    let mut failed_rows = 0usize;
    let mut max_risk = ResidualRisk::Low;
    for row in rows.iter() {
        if row.safety_pass && row.temporal_pass && row.security_pass && row.parity_pass {
            passed_rows += 1;
        } else {
            failed_rows += 1;
        }
        max_risk = match (max_risk, row.residual_risk) {
            (ResidualRisk::High, _) | (_, ResidualRisk::High) => ResidualRisk::High,
            (ResidualRisk::Medium, _) | (_, ResidualRisk::Medium) => ResidualRisk::Medium,
            _ => ResidualRisk::Low,
        };
    }
    ScenarioCertificationReport {
        pass: failed_rows == 0 && !matches!(max_risk, ResidualRisk::High),
        passed_rows,
        failed_rows,
        max_residual_risk: max_risk,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        certify_scenarios, evaluate_seu, parity_gate, redundant_vote, verify_secure_chain,
        DeterministicMode, FaultEvent, FaultSymptom, FdirEngine, HealthGovernor, HealthInput,
        ParityThresholds, ResidualRisk, RuntimeDecisionDigest, RuntimeTelemetry, SafetyInvariants,
        ScenarioCertificationRow, SecureMissionInput, SeuSlotState, TemporalContracts,
        TemporalSample, TwinHardwareMetrics,
    };
    use heapless::Vec;

    #[test]
    fn fdir_maps_fault_to_safe_recovery_script() {
        let fdir = FdirEngine;
        let report = fdir.isolate_and_plan(FaultEvent {
            symptom: FaultSymptom::AckTimeoutBurst,
            severity: 8,
        });
        assert!(!report.actions.is_empty());
        assert!(report
            .actions
            .iter()
            .any(|item| matches!(item, super::RecoveryAction::ResetRadio)));
    }

    #[test]
    fn invariants_block_unsafe_runtime_state() {
        let limits = SafetyInvariants {
            max_tx_power_dbm: 17,
            max_duty_cycle_permille: 150,
            max_latency_ms: 300,
            max_thermal_c: 85,
            max_queue_depth: 8,
        };
        let check = limits.evaluate(RuntimeTelemetry {
            tx_power_dbm: 20,
            duty_cycle_permille: 200,
            latency_ms: 200,
            thermal_c: 70,
            queue_depth: 12,
        });
        assert!(!check.pass);
        assert!(check.violations.len() >= 2);
    }

    #[test]
    fn redundant_runtime_rolls_back_on_mismatch() {
        let primary = RuntimeDecisionDigest {
            profile_id: 1,
            priority_id: 100,
            mode: DeterministicMode::Nominal,
        };
        let shadow = RuntimeDecisionDigest {
            profile_id: 2,
            priority_id: 100,
            mode: DeterministicMode::Nominal,
        };
        let checkpoint = RuntimeDecisionDigest {
            profile_id: 9,
            priority_id: 300,
            mode: DeterministicMode::Survival,
        };
        let verdict = redundant_vote(primary, shadow, checkpoint);
        assert!(verdict.disagreement);
        assert!(verdict.rollback_used);
        assert_eq!(verdict.accepted.profile_id, 9);
    }

    #[test]
    fn governor_degrades_and_recovers_deterministically() {
        let mut governor = HealthGovernor::default();
        let degraded = governor.update(HealthInput {
            ack_ratio: 0.2,
            error_rate: 0.4,
            queue_depth_norm: 0.8,
            thermal_margin_norm: 0.1,
        });
        assert!(matches!(
            degraded,
            DeterministicMode::Survival | DeterministicMode::Silent
        ));
        let recovered = governor.update(HealthInput {
            ack_ratio: 0.98,
            error_rate: 0.01,
            queue_depth_norm: 0.1,
            thermal_margin_norm: 0.9,
        });
        assert_eq!(recovered, DeterministicMode::Nominal);
    }

    #[test]
    fn temporal_contracts_detect_sla_breaks() {
        let contracts = TemporalContracts {
            detect_us_max: 120,
            decision_us_max: 240,
            apply_us_max: 300,
            recover_us_max: 2000,
        };
        let report = contracts.evaluate(TemporalSample {
            detect_us: 90,
            decision_us: 300,
            apply_us: 280,
            recover_us: 1500,
        });
        assert!(!report.pass);
        assert!(!report.decision_ok);
    }

    #[test]
    fn secure_chain_and_parity_matrix_gate() {
        let secure = verify_secure_chain(SecureMissionInput {
            signature_ok: true,
            boot_chain_ok: true,
            key_epoch: 7,
            min_allowed_epoch: 6,
            key_revoked: false,
        });
        assert!(secure.pass);

        let parity = parity_gate(
            TwinHardwareMetrics {
                pdr: 0.91,
                latency_ms: 110.0,
                recover_ms: 420.0,
            },
            TwinHardwareMetrics {
                pdr: 0.88,
                latency_ms: 130.0,
                recover_ms: 450.0,
            },
            ParityThresholds {
                pdr_delta_max: 0.05,
                latency_delta_max_ms: 25.0,
                recover_delta_max_ms: 50.0,
            },
        );
        assert!(parity.pass);

        let seu = evaluate_seu(
            SeuSlotState {
                crc_ok: true,
                ecc_corrected_bits: 3,
                bitflip_count: 2,
            },
            SeuSlotState {
                crc_ok: true,
                ecc_corrected_bits: 5,
                bitflip_count: 1,
            },
            100,
            200,
        );
        assert!(seu.pass);
        assert!(seu.scrub_required);

        let mut rows = Vec::<ScenarioCertificationRow, 4>::new();
        let _ = rows.push(ScenarioCertificationRow {
            scenario_id: 1,
            safety_pass: true,
            temporal_pass: true,
            security_pass: true,
            parity_pass: true,
            residual_risk: ResidualRisk::Low,
        });
        let report = certify_scenarios(&rows);
        assert!(report.pass);
        assert_eq!(report.failed_rows, 0);
    }
}
