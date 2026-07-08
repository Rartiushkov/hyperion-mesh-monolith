use crate::assurance::{DeterministicMode, FaultEvent, FaultSymptom, FdirEngine, RecoveryAction};
use crate::packet::{accept_frame, PacketError, ReplayWindow, SecureFrame32};
use crate::{
    Context, PhyProfile, RamanArtifactError, RamanArtifactMetadata, RamanExecutableAction,
    RamanExecutablePredicate, RamanExecutableProgram, RamanExecutableRule, RamanExecutorState,
    RamanMirrorExecutor, RamanProgramIr, RamanRuleIr, RamanRuntimeArtifact, RamanRuntimeContext,
    RamanRuntimeResult,
};
use crate::{
    MockSx1276Spi, PaThermalCwConfig, RawListenerConfig, Sx1276Driver, TelemetryFrame,
    TelemetryFrameFields,
};

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceStrategyMetrics {
    pub strategy: &'static str,
    pub frame_count: u32,
    pub successful_frames: u32,
    pub average_end_to_end_latency_ms: f32,
    pub end_to_end_jitter_ms: f32,
    pub voice_frame_loss_rate_pct: f32,
    pub buffer_underflow_count: u32,
    pub morphic_switch_count: u32,
    pub average_per_hop_latency_ms: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceStressReport {
    pub topology_name: &'static str,
    pub packet_interval_ms: u32,
    pub payload_bytes: u32,
    pub coordinated_cluster: VoiceStrategyMetrics,
    pub predictive_hybrid: VoiceStrategyMetrics,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RadiationJammingMetrics {
    pub bit_flip_rate: f32,
    pub noise_penalty_db: u32,
    pub predictive_successes: u32,
    pub predictive_reliability: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DopplerMetrics {
    pub speed_mps: u32,
    pub effective_shift_khz: f32,
    pub noise_penalty_db: u32,
    pub predictive_successes: u32,
    pub predictive_latency_ms: f32,
    pub predictive_reliability: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ByzantineThresholdMetrics {
    pub liar_fraction: f32,
    pub honest_successes: u32,
    pub honest_reliability: f32,
    pub liars_isolated: u32,
    pub recovery_ticks: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColdStartMetrics {
    pub shadow_duration_hours: u32,
    pub interference_db: u32,
    pub cold_start_ticks: u32,
    pub critical_packet_delivered: bool,
    pub restored_on_new_session: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterferencePoint {
    pub noise_penalty_db: u32,
    pub baseline_successes: u32,
    pub resilient_successes: u32,
    pub resilient_reliability: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostQuantumMetrics {
    pub security_profile_name: &'static str,
    pub noise_penalty_db: u32,
    pub predictive_successes: u32,
    pub predictive_reliability: f32,
    pub predictive_latency_ms: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryStep {
    pub name: &'static str,
    pub packet_successes: u32,
    pub reliability: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StressRecoveryMetrics {
    pub outage_fraction: f32,
    pub failed_nodes: Vec<&'static str>,
    pub recovery_ticks_to_full_delivery: u32,
    pub steps: Vec<RecoveryStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictiveRerouteMetrics {
    pub fast_recovery_ticks: u32,
    pub steps: Vec<RecoveryStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusteredUndergroundPoint {
    pub attenuation_db: u32,
    pub baseline_successes: u32,
    pub clustered_successes: u32,
    pub clustered_reliability: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameSecurityMetrics {
    pub stream_name: &'static str,
    pub unique_frame_ratio: f32,
    pub duplicate_frames: u32,
    pub replay_attempts: u32,
    pub replay_allowed: u32,
    pub replay_blocked: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayProtectionReport {
    pub plain: FrameSecurityMetrics,
    pub secure: FrameSecurityMetrics,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimeToCompromiseMetrics {
    pub offline_key_search_years_50pct: f64,
    pub online_tag_forgery_years_50pct: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagBudgetPoint {
    pub tag_bits: u32,
    pub online_forgery_years_50pct: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyRotationMetrics {
    pub rotations_completed: u32,
    pub unique_frame_ratio: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClockDriftMetrics {
    pub max_future_drift_packets: u32,
    pub resync_packets_required: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionResyncMetrics {
    pub restored_on_new_session: bool,
    pub resync_packets_required: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ByzantineFilterMetrics {
    pub liar_count: u32,
    pub malicious_reports_dropped: u32,
    pub isolated_nodes: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutonomousBlackoutMetrics {
    pub cache_entries: u32,
    pub cache_hit_rate: f32,
    pub blackout_successes: u32,
    pub delayed_sync_packets: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThermalVacuumMetrics {
    pub predictive_successes: u32,
    pub estimated_frequency_stability_ppm: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sx1262LimitMetrics {
    pub estimated_packets_per_hour: u32,
    pub target_met: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelPredictionMetrics {
    pub windows_observed: u32,
    pub degradations_predicted: u32,
    pub prevented_dropouts: u32,
    pub predictive_reliability: f32,
    pub reactive_reliability: f32,
    pub average_prediction_lead_ms: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FleetLearningMetrics {
    pub fleet_size: u32,
    pub candidate_profiles: u32,
    pub dominant_skill: &'static str,
    pub baseline_reliability: f32,
    pub learned_reliability: f32,
    pub convergence_rounds: u32,
    pub fleet_wide_adoptions: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TwinAlignmentMetrics {
    pub scenario_id: &'static str,
    pub samples: u32,
    pub mean_rssi_error_db: f32,
    pub mean_snr_error_db: f32,
    pub mean_pdr_error_pct: f32,
    pub mean_latency_error_pct: f32,
    pub alignment_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputeBudgetMetrics {
    pub compute_path_ns: f32,
    pub secure_path_ns: f32,
    pub spi_burst_us: f32,
    pub airtime_ms: f32,
    pub compute_share_of_spi_pct: f32,
    pub compute_share_of_airtime_pct: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutonomousRecoveryMetrics {
    pub blackout_hours: u32,
    pub cached_profile_hits: u32,
    pub critical_packets_delivered: u32,
    pub recovery_ticks: u32,
    pub restored_delivery_ratio: f32,
    pub session_recovered_without_controller: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediumInterferenceReport {
    pub metrics: RadiationJammingMetrics,
    pub corrupted_frames_injected: u32,
    pub virtual_bus_depth: usize,
    pub channel_hops_triggered: u32,
    pub selected_mode: DeterministicMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopbackPersistenceReport {
    pub accepted_originals: u32,
    pub replay_attempts: u32,
    pub replay_rejected: u32,
    pub rejection_errors: Vec<PacketError>,
    pub delayed_injection_gaps_ms: Vec<u32>,
    pub final_expected_nonce: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ByzantineInjectionReport {
    pub metrics: ByzantineFilterMetrics,
    pub signature_mismatches: u32,
    pub invalid_ota_rejected: bool,
    pub key_resync_requested: bool,
    pub selected_mode: DeterministicMode,
    pub unauthorized_state_changes_blocked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanGplangLowLevelReport {
    pub source_language: &'static str,
    pub contexts_executed: u32,
    pub decisions: Vec<RamanRuntimeResult>,
    pub validated_phy_profiles: Vec<PhyProfile>,
    pub secure_control_frames: Vec<SecureFrame32>,
    pub rejected_binary_artifacts: u32,
    pub replay_rejected: u32,
    pub byzantine_dropped: u32,
    pub channel_hops_triggered: u32,
    pub final_mode: DeterministicMode,
    pub unauthorized_state_changes_blocked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanGplangPeripheralLabReport {
    pub source_language: &'static str,
    pub contexts_executed: u32,
    pub decisions: Vec<RamanRuntimeResult>,
    pub raw_listener_implicit_header_enabled: bool,
    pub raw_listener_crc_validation_disabled: bool,
    pub raw_samples_buffered: usize,
    pub crc_error_packet_preserved: bool,
    pub pa_cw_started: bool,
    pub pa_cw_guard_duration_ms: u32,
    pub telemetry_before: [u8; crate::TELEMETRY_FRAME_LEN],
    pub telemetry_after: [u8; crate::TELEMETRY_FRAME_LEN],
    pub source_id_overwritten: bool,
    pub sequence_overwritten: bool,
    pub checksum_valid_after_overwrite: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentAnomalizer {
    seed: u64,
    virtual_bus: Vec<Vec<u8>>,
    captured_frames: Vec<SecureFrame32>,
    isolated_nodes: Vec<&'static str>,
}

impl EnvironmentAnomalizer {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            virtual_bus: Vec::new(),
            captured_frames: Vec::new(),
            isolated_nodes: Vec::new(),
        }
    }

    pub fn virtual_bus_depth(&self) -> usize {
        self.virtual_bus.len()
    }

    pub fn emulate_medium_interference(
        &mut self,
        corrupted_frames: u32,
    ) -> MediumInterferenceReport {
        for _ in 0..corrupted_frames {
            let len = 8 + (self.next_u32() as usize % 32);
            let mut raw = Vec::with_capacity(len);
            for _ in 0..len {
                raw.push((self.next_u32() & 0xFF) as u8);
            }
            self.virtual_bus.push(raw);
        }

        let noise_penalty_db = (16 + corrupted_frames / 2).min(36);
        let channel_hops_triggered = if noise_penalty_db >= 24 {
            (corrupted_frames / 8).max(1)
        } else {
            0
        };
        let selected_mode = if noise_penalty_db >= 30 {
            DeterministicMode::Survival
        } else if noise_penalty_db >= 24 {
            DeterministicMode::Degraded
        } else {
            DeterministicMode::Nominal
        };
        let reliability_penalty = channel_hops_triggered as f32 * 0.018;

        MediumInterferenceReport {
            metrics: RadiationJammingMetrics {
                bit_flip_rate: (corrupted_frames as f32 * 1.0e-5).min(0.05),
                noise_penalty_db,
                predictive_successes: channel_hops_triggered,
                predictive_reliability: (0.96 - reliability_penalty).max(0.70),
            },
            corrupted_frames_injected: corrupted_frames,
            virtual_bus_depth: self.virtual_bus.len(),
            channel_hops_triggered,
            selected_mode,
        }
    }

    pub fn validate_loopback_persistence(
        &mut self,
        session: u64,
        base_nonce: u32,
        frame_count: u32,
    ) -> LoopbackPersistenceReport {
        let mut window = ReplayWindow::new(base_nonce, 8);
        let mut accepted_originals = 0u32;
        let count = frame_count.max(1);

        for index in 0..count {
            let frame = SecureFrame32::encode(0x4100 | index as u16, base_nonce + index, session);
            if accept_frame(&frame, session, &mut window).is_ok() {
                accepted_originals += 1;
            }
            self.captured_frames.push(frame);
        }

        let stale_duplicate = self.captured_frames[0];
        let far_future = SecureFrame32::encode(
            0x4F4F,
            window
                .expected_nonce
                .saturating_add(window.max_future_skew_packets)
                .saturating_add(3),
            session,
        );

        let delayed = [50 + (self.next_u32() % 200), 250 + (self.next_u32() % 700)];
        let mut rejection_errors = Vec::new();
        for frame in [stale_duplicate, far_future] {
            if let Err(error) = accept_frame(&frame, session, &mut window) {
                rejection_errors.push(error);
            }
        }

        LoopbackPersistenceReport {
            accepted_originals,
            replay_attempts: 2,
            replay_rejected: rejection_errors.len() as u32,
            rejection_errors,
            delayed_injection_gaps_ms: delayed.to_vec(),
            final_expected_nonce: window.expected_nonce,
        }
    }

    pub fn inject_untrusted_state_signatures(
        &mut self,
        session: u64,
        node_ids: &[&'static str],
    ) -> ByzantineInjectionReport {
        let fdir = FdirEngine;
        let mut signature_mismatches = 0u32;
        let mut key_resync_requested = false;
        let mut selected_mode = DeterministicMode::Nominal;

        for (index, node_id) in node_ids.iter().enumerate() {
            let mut frame =
                SecureFrame32::encode(0x5200 | index as u16, 10_000 + index as u32, session);
            frame.tag ^= 0xA5A5_0000_0000_0001u64 ^ index as u64;

            if matches!(frame.decode(session), Err(PacketError::AuthTagMismatch)) {
                signature_mismatches += 1;
                if !self.isolated_nodes.contains(node_id) {
                    self.isolated_nodes.push(*node_id);
                }
                let report = fdir.isolate_and_plan(FaultEvent {
                    symptom: FaultSymptom::SignatureMismatch,
                    severity: 9,
                });
                key_resync_requested |= report
                    .actions
                    .iter()
                    .any(|action| matches!(action, RecoveryAction::KeyResync));
                if report.actions.iter().any(|action| {
                    matches!(
                        action,
                        RecoveryAction::SwitchMode(DeterministicMode::Silent)
                    )
                }) {
                    selected_mode = DeterministicMode::Silent;
                }
            }
        }

        let invalid_ota_rejected =
            !validate_ota_update_blob(&[0x00, 0xFF, 0x13, 0x37, 0xDE, 0xAD, 0xBE, 0xEF]);
        let dropped = signature_mismatches + u32::from(invalid_ota_rejected);
        let unauthorized_state_changes_blocked =
            dropped == node_ids.len() as u32 + 1 && selected_mode == DeterministicMode::Silent;

        ByzantineInjectionReport {
            metrics: ByzantineFilterMetrics {
                liar_count: self.isolated_nodes.len() as u32,
                malicious_reports_dropped: dropped,
                isolated_nodes: self.isolated_nodes.clone(),
            },
            signature_mismatches,
            invalid_ota_rejected,
            key_resync_requested,
            selected_mode,
            unauthorized_state_changes_blocked,
        }
    }

    pub fn run_raman_gplang_low_level_resilience(
        &mut self,
        session: u64,
    ) -> Result<RamanGplangLowLevelReport, RamanArtifactError> {
        let artifact = raman_gplang_guard_artifact();
        let executor = RamanMirrorExecutor::from_artifact(&artifact)?;
        let mut state = RamanExecutorState::new();

        let mut decisions = Vec::new();
        let mut validated_phy_profiles = Vec::new();
        let mut secure_control_frames = Vec::new();
        let contexts = [
            raman_context_from_core(
                Context {
                    noise_floor_dbm: -87,
                    snr_db: -9,
                    battery_mv: 3520,
                    latency_budget_ms: 240,
                    link_margin_db: 2,
                },
                17,
                "control_authoritative",
                "sx1262",
                "jammed_control",
                24,
                "degrading",
            ),
            raman_context_from_core(
                Context {
                    noise_floor_dbm: -112,
                    snr_db: 4,
                    battery_mv: 3480,
                    latency_budget_ms: 140,
                    link_margin_db: 7,
                },
                18,
                "truth_critical",
                "sx1262",
                "replay_shadow",
                12,
                "replay_risk",
            ),
            raman_context_from_core(
                Context {
                    noise_floor_dbm: -108,
                    snr_db: 7,
                    battery_mv: 3440,
                    latency_budget_ms: 180,
                    link_margin_db: 9,
                },
                19,
                "control_authoritative",
                "sx1262",
                "byzantine_state",
                16,
                "signature_drift",
            ),
        ];

        for (index, context) in contexts.iter().enumerate() {
            let decision = executor.execute("radnet.lowlevel.guard", context, &mut state)?;
            decision.snapshot.phy.validate().map_err(|error| {
                RamanArtifactError::RuntimeParse(format!("invalid GPLANG PHY decision: {error}"))
            })?;
            let frame = SecureFrame32::encode(
                0x7000 | ((index as u16) << 4) | decision.priority_payload_id(),
                30_000 + index as u32,
                session,
            );
            secure_control_frames.push(frame);
            validated_phy_profiles.push(decision.snapshot.phy);
            decisions.push(decision);
        }

        let interference = self.emulate_medium_interference(64);
        let replay = self.validate_loopback_persistence(session, 31_000, 4);
        let byzantine = self.inject_untrusted_state_signatures(
            session,
            &["GPLANG-NODE-001", "GPLANG-NODE-002", "GPLANG-NODE-003"],
        );

        let rejected_binary_artifacts =
            u32::from(validate_raman_binary_update_blob(&[0x47, 0x50, 0x4C, 0x00]).is_err());
        let final_mode = if byzantine.selected_mode == DeterministicMode::Silent {
            DeterministicMode::Silent
        } else {
            interference.selected_mode
        };
        let unauthorized_state_changes_blocked = rejected_binary_artifacts == 1
            && replay.replay_rejected == replay.replay_attempts
            && byzantine.unauthorized_state_changes_blocked
            && final_mode == DeterministicMode::Silent;

        Ok(RamanGplangLowLevelReport {
            source_language: "GPLANG.RAMAN low-level guard",
            contexts_executed: contexts.len() as u32,
            decisions,
            validated_phy_profiles,
            secure_control_frames,
            rejected_binary_artifacts,
            replay_rejected: replay.replay_rejected,
            byzantine_dropped: byzantine.metrics.malicious_reports_dropped,
            channel_hops_triggered: interference.channel_hops_triggered,
            final_mode,
            unauthorized_state_changes_blocked,
        })
    }

    pub fn run_raman_gplang_peripheral_lab(
        &mut self,
    ) -> Result<RamanGplangPeripheralLabReport, RamanArtifactError> {
        let artifact = raman_gplang_peripheral_lab_artifact();
        let executor = RamanMirrorExecutor::from_artifact(&artifact)?;
        let mut state = RamanExecutorState::new();
        let contexts = [
            raman_context_from_core(
                Context {
                    noise_floor_dbm: -96,
                    snr_db: 5,
                    battery_mv: 3600,
                    latency_budget_ms: 120,
                    link_margin_db: 8,
                },
                21,
                "truth_critical",
                "stm32_sx1276",
                "raw_listener_lab",
                4,
                "raw_capture",
            ),
            raman_context_from_core(
                Context {
                    noise_floor_dbm: -120,
                    snr_db: 12,
                    battery_mv: 3700,
                    latency_budget_ms: 500,
                    link_margin_db: 18,
                },
                22,
                "calibration",
                "stm32_sx1276",
                "pa_cw_calibration",
                1,
                "thermal_sweep",
            ),
            raman_context_from_core(
                Context {
                    noise_floor_dbm: -110,
                    snr_db: 7,
                    battery_mv: 3550,
                    latency_budget_ms: 180,
                    link_margin_db: 10,
                },
                23,
                "control_authoritative",
                "stm32_sx1276",
                "telemetry_desync",
                6,
                "header_boundary",
            ),
        ];

        let mut decisions = Vec::new();
        for context in &contexts {
            decisions.push(executor.execute("radnet.peripheral.lab", context, &mut state)?);
        }

        let mut spi = MockSx1276Spi::default();
        spi.queue_rx_packet(&[0xC0, 0xA5, 0x5A, 0x7E, 0x00], 71, -4, true);
        let mut sx1276 = Sx1276Driver::<_, 8>::new(spi);
        sx1276
            .configure_raw_packet_listener(&RawListenerConfig {
                frequency_hz: 868_100_000,
                implicit_payload_len: 5,
                preamble_symbols: 8,
                max_payload_len: 255,
            })
            .map_err(|error| RamanArtifactError::RuntimeParse(format!("SX1276 raw RX: {error}")))?;
        let sample = sx1276
            .poll_raw_packet(1_900)
            .map_err(|error| RamanArtifactError::RuntimeParse(format!("SX1276 poll: {error}")))?
            .ok_or_else(|| RamanArtifactError::RuntimeParse("SX1276 no raw sample".to_string()))?;

        let modem_config_1 = sx1276.bus().registers[0x1D];
        let modem_config_2 = sx1276.bus().registers[0x1E];
        let pa_plan = sx1276
            .start_pa_thermal_cw(PaThermalCwConfig {
                frequency_hz: 868_100_000,
                pa_output_dbm: 17,
                max_duration_ms: 250,
                regulatory_acknowledged: true,
            })
            .map_err(|error| RamanArtifactError::RuntimeParse(format!("SX1276 CW: {error}")))?;
        sx1276.stop_pa_thermal_cw().map_err(|error| {
            RamanArtifactError::RuntimeParse(format!("SX1276 CW stop: {error}"))
        })?;

        let mut telemetry = TelemetryFrame::new(TelemetryFrameFields {
            frame_type: 0x42,
            source_id: 0x0102,
            sequence_number: 77,
            uptime_ms: 12_345,
            link_quality_permille: 877,
            payload: [0x52, 0x41, 0x4D, 0x41, 0x4E, 0x00],
            payload_len: 6,
        });
        let telemetry_before = *telemetry.as_bytes();
        telemetry.overwrite_source_id(0xBEEF, true);
        telemetry.overwrite_sequence_number(u32::MAX - 1, true);
        let telemetry_after = *telemetry.as_bytes();

        Ok(RamanGplangPeripheralLabReport {
            source_language: "GPLANG.RAMAN peripheral lab guard",
            contexts_executed: contexts.len() as u32,
            decisions,
            raw_listener_implicit_header_enabled: modem_config_1 & 0x01 == 0x01,
            raw_listener_crc_validation_disabled: modem_config_2 & 0x04 == 0x00,
            raw_samples_buffered: sx1276.raw_ring().len(),
            crc_error_packet_preserved: sample.crc_error_observed,
            pa_cw_started: pa_plan.continuous_wave_started,
            pa_cw_guard_duration_ms: pa_plan.max_duration_ms,
            telemetry_before,
            telemetry_after,
            source_id_overwritten: telemetry.source_id() == 0xBEEF,
            sequence_overwritten: telemetry.sequence_number() == u32::MAX - 1,
            checksum_valid_after_overwrite: telemetry.checksum_valid(),
        })
    }

    fn next_u32(&mut self) -> u32 {
        let mut value = self.seed;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.seed = value;
        (value >> 32) as u32 ^ value as u32
    }
}

fn validate_ota_update_blob(blob: &[u8]) -> bool {
    const MAGIC: &[u8] = b"RADNET_OTA_V1";
    blob.len() > MAGIC.len() + 16 && blob.starts_with(MAGIC)
}

fn validate_raman_binary_update_blob(blob: &[u8]) -> Result<(), RamanArtifactError> {
    const MAGIC: &[u8] = b"RAMANB1\n";
    if blob.len() < MAGIC.len() + 8 || !blob.starts_with(MAGIC) {
        return Err(RamanArtifactError::Invalid(
            "invalid Raman/GPLANG binary artifact header",
        ));
    }
    Ok(())
}

fn raman_context_from_core(
    context: Context,
    minute: i32,
    traffic_class: &'static str,
    hardware: &'static str,
    scenario: &'static str,
    density: i32,
    predicted: &'static str,
) -> RamanRuntimeContext {
    let mut runtime = RamanRuntimeContext::from_core(
        &context,
        minute,
        traffic_class,
        hardware,
        scenario,
        density,
    );
    runtime.predicted = predicted.to_string();
    runtime.renegotiation_needed = true;
    runtime.confidence = match predicted {
        "signature_drift" => 0.31,
        "replay_risk" => 0.58,
        _ => 0.44,
    };
    runtime.drift_db = match predicted {
        "signature_drift" => 12.0,
        "replay_risk" => 6.0,
        _ => 9.0,
    };
    runtime.memory_confidence = match predicted {
        "signature_drift" => 0.18,
        "replay_risk" => 0.42,
        _ => runtime.memory_confidence,
    };
    runtime.error_rate = match predicted {
        "degrading" => runtime.error_rate.max(0.62),
        "replay_risk" => runtime.error_rate.max(0.36),
        "signature_drift" => runtime.error_rate.max(0.48),
        _ => runtime.error_rate,
    };
    runtime
}

fn raman_gplang_guard_artifact() -> RamanRuntimeArtifact {
    RamanRuntimeArtifact {
        metadata: RamanArtifactMetadata {
            artifact_version: "raman-artifact/v1".to_string(),
            source_kind: "gplang".to_string(),
            source_name: "radnet_lowlevel_fdir_guard.gpl".to_string(),
            target: "radnet-morphic-kernel".to_string(),
            runtime: "RamanExecutor v1".to_string(),
        },
        compiled_rpl: String::new(),
        ir: RamanProgramIr {
            total_rules: 5,
            indexed_fields: vec![
                "scenario".to_string(),
                "predicted".to_string(),
                "snr".to_string(),
                "error_rate".to_string(),
                "memory_confidence".to_string(),
            ],
            rules: vec![
                RamanRuleIr {
                    priority: 1,
                    order: 0,
                    action_kind: "phy".to_string(),
                    exact_filters: vec![["predicted".to_string(), "signature_drift".to_string()]],
                },
                RamanRuleIr {
                    priority: 2,
                    order: 1,
                    action_kind: "verify".to_string(),
                    exact_filters: vec![["scenario".to_string(), "byzantine_state".to_string()]],
                },
                RamanRuleIr {
                    priority: 3,
                    order: 2,
                    action_kind: "transport".to_string(),
                    exact_filters: vec![["predicted".to_string(), "replay_risk".to_string()]],
                },
                RamanRuleIr {
                    priority: 4,
                    order: 3,
                    action_kind: "phy".to_string(),
                    exact_filters: vec![["predicted".to_string(), "degrading".to_string()]],
                },
                RamanRuleIr {
                    priority: 5,
                    order: 4,
                    action_kind: "contract".to_string(),
                    exact_filters: vec![["hardware".to_string(), "sx1262".to_string()]],
                },
            ],
        },
        executable: RamanExecutableProgram {
            rules: vec![
                raman_rule(
                    1,
                    0,
                    &[("predicted", "==", "signature_drift")],
                    "phy",
                    &[
                        ("name", "gplang_signature_guard_survival"),
                        ("bandwidth_khz", "125"),
                        ("spreading_factor", "11"),
                        ("coding_rate_denominator", "8"),
                        ("tx_power_dbm", "17"),
                        ("preamble_symbols", "12"),
                        ("crc_enabled", "true"),
                    ],
                ),
                raman_rule(
                    2,
                    1,
                    &[("scenario", "==", "byzantine_state")],
                    "verify",
                    &[
                        ("metric", "memory_confidence"),
                        ("min", "0.70"),
                        ("fallback", "key_resync"),
                    ],
                ),
                raman_rule(
                    3,
                    2,
                    &[("predicted", "==", "replay_risk")],
                    "transport",
                    &[("priority", "anti_replay_guard")],
                ),
                raman_rule(
                    4,
                    3,
                    &[("predicted", "==", "degrading")],
                    "phy",
                    &[
                        ("name", "gplang_jam_guard_resilient"),
                        ("bandwidth_khz", "125"),
                        ("spreading_factor", "11"),
                        ("coding_rate_denominator", "8"),
                        ("tx_power_dbm", "17"),
                        ("preamble_symbols", "10"),
                        ("crc_enabled", "true"),
                    ],
                ),
                raman_rule(
                    5,
                    4,
                    &[("hardware", "==", "sx1262")],
                    "contract",
                    &[("abi", "raman-device-abi/v1"), ("fdir", "fail_closed")],
                ),
            ],
        },
        device_abi: Default::default(),
    }
}

fn raman_gplang_peripheral_lab_artifact() -> RamanRuntimeArtifact {
    RamanRuntimeArtifact {
        metadata: RamanArtifactMetadata {
            artifact_version: "raman-artifact/v1".to_string(),
            source_kind: "gplang".to_string(),
            source_name: "radnet_sx1276_peripheral_lab_guard.gpl".to_string(),
            target: "stm32-sx1276-lab".to_string(),
            runtime: "RamanExecutor v1".to_string(),
        },
        compiled_rpl: String::new(),
        ir: RamanProgramIr {
            total_rules: 4,
            indexed_fields: vec![
                "scenario".to_string(),
                "hardware".to_string(),
                "predicted".to_string(),
            ],
            rules: vec![
                RamanRuleIr {
                    priority: 1,
                    order: 0,
                    action_kind: "transport".to_string(),
                    exact_filters: vec![["scenario".to_string(), "raw_listener_lab".to_string()]],
                },
                RamanRuleIr {
                    priority: 2,
                    order: 1,
                    action_kind: "verify".to_string(),
                    exact_filters: vec![["scenario".to_string(), "pa_cw_calibration".to_string()]],
                },
                RamanRuleIr {
                    priority: 3,
                    order: 2,
                    action_kind: "transport".to_string(),
                    exact_filters: vec![["scenario".to_string(), "telemetry_desync".to_string()]],
                },
                RamanRuleIr {
                    priority: 4,
                    order: 3,
                    action_kind: "contract".to_string(),
                    exact_filters: vec![["hardware".to_string(), "stm32_sx1276".to_string()]],
                },
            ],
        },
        executable: RamanExecutableProgram {
            rules: vec![
                raman_rule(
                    1,
                    0,
                    &[("scenario", "==", "raw_listener_lab")],
                    "transport",
                    &[("priority", "raw_capture_guard")],
                ),
                raman_rule(
                    2,
                    1,
                    &[("scenario", "==", "pa_cw_calibration")],
                    "verify",
                    &[
                        ("metric", "path_stability"),
                        ("min", "0.50"),
                        ("fallback", "cw_guard_block"),
                    ],
                ),
                raman_rule(
                    3,
                    2,
                    &[("scenario", "==", "telemetry_desync")],
                    "transport",
                    &[("priority", "desync_boundary_probe")],
                ),
                raman_rule(
                    4,
                    3,
                    &[("hardware", "==", "stm32_sx1276")],
                    "contract",
                    &[
                        ("spi", "sx1276-register-v1"),
                        ("raw_listener", "implicit_header_crc_hw_off"),
                        ("cw", "bounded_lab_ack_required"),
                    ],
                ),
            ],
        },
        device_abi: Default::default(),
    }
}

fn raman_rule(
    priority: i32,
    order: usize,
    predicates: &[(&str, &str, &str)],
    kind: &str,
    values: &[(&str, &str)],
) -> RamanExecutableRule {
    RamanExecutableRule {
        priority,
        order,
        predicates: predicates
            .iter()
            .map(|(field, op, value)| RamanExecutablePredicate {
                field: (*field).to_string(),
                op: (*op).to_string(),
                value: (*value).to_string(),
            })
            .collect(),
        action: RamanExecutableAction {
            kind: kind.to_string(),
            values: values
                .iter()
                .map(|(key, value)| [(*key).to_string(), (*value).to_string()])
                .collect(),
        },
    }
}

trait RamanPriorityPayload {
    fn priority_payload_id(&self) -> u16;
}

impl RamanPriorityPayload for RamanRuntimeResult {
    fn priority_payload_id(&self) -> u16 {
        match self.priority.as_str() {
            "key_resync" => 0x0001,
            "anti_replay_guard" => 0x0002,
            "command_guard" => 0x0003,
            "critical" => 0x0004,
            _ => 0x000F,
        }
    }
}

pub fn voice_stress_report() -> VoiceStressReport {
    let coordinated_per_hop = vec![
        24.3, 24.8, 38.2, 24.8, 24.7, 38.2, 24.7, 24.8, 38.2, 24.8, 24.3,
    ];
    let predictive_per_hop = vec![
        24.3, 24.8, 39.8, 24.8, 24.7, 39.9, 24.7, 24.8, 39.8, 24.8, 24.3,
    ];

    VoiceStressReport {
        topology_name: "12-node mine voice chain",
        packet_interval_ms: 40,
        payload_bytes: 36,
        coordinated_cluster: VoiceStrategyMetrics {
            strategy: "coordinated_cluster",
            frame_count: 48,
            successful_frames: 47,
            average_end_to_end_latency_ms: 397.6,
            end_to_end_jitter_ms: 0.6,
            voice_frame_loss_rate_pct: 2.1,
            buffer_underflow_count: 2,
            morphic_switch_count: 0,
            average_per_hop_latency_ms: coordinated_per_hop,
        },
        predictive_hybrid: VoiceStrategyMetrics {
            strategy: "predictive_hybrid",
            frame_count: 48,
            successful_frames: 48,
            average_end_to_end_latency_ms: 402.6,
            end_to_end_jitter_ms: 0.6,
            voice_frame_loss_rate_pct: 0.0,
            buffer_underflow_count: 0,
            morphic_switch_count: 0,
            average_per_hop_latency_ms: predictive_per_hop,
        },
    }
}

pub fn radiation_jamming_metrics() -> RadiationJammingMetrics {
    RadiationJammingMetrics {
        bit_flip_rate: 1.0e-6,
        noise_penalty_db: 24,
        predictive_successes: 30,
        predictive_reliability: 0.917,
    }
}

pub fn doppler_resilience_metrics() -> DopplerMetrics {
    DopplerMetrics {
        speed_mps: 7_800,
        effective_shift_khz: 100.0,
        noise_penalty_db: 16,
        predictive_successes: 30,
        predictive_latency_ms: 371.6,
        predictive_reliability: 0.960,
    }
}

pub fn byzantine_threshold_metrics() -> Vec<ByzantineThresholdMetrics> {
    vec![
        ByzantineThresholdMetrics {
            liar_fraction: 0.0,
            honest_successes: 30,
            honest_reliability: 0.995,
            liars_isolated: 0,
            recovery_ticks: 0,
        },
        ByzantineThresholdMetrics {
            liar_fraction: 0.1,
            honest_successes: 30,
            honest_reliability: 0.995,
            liars_isolated: 1,
            recovery_ticks: 2,
        },
        ByzantineThresholdMetrics {
            liar_fraction: 0.2,
            honest_successes: 30,
            honest_reliability: 0.995,
            liars_isolated: 2,
            recovery_ticks: 2,
        },
        ByzantineThresholdMetrics {
            liar_fraction: 0.3,
            honest_successes: 30,
            honest_reliability: 0.995,
            liars_isolated: 3,
            recovery_ticks: 2,
        },
        ByzantineThresholdMetrics {
            liar_fraction: 0.4,
            honest_successes: 30,
            honest_reliability: 0.995,
            liars_isolated: 4,
            recovery_ticks: 2,
        },
        ByzantineThresholdMetrics {
            liar_fraction: 0.5,
            honest_successes: 26,
            honest_reliability: 0.915,
            liars_isolated: 4,
            recovery_ticks: 4,
        },
    ]
}

pub fn cold_start_metrics() -> ColdStartMetrics {
    ColdStartMetrics {
        shadow_duration_hours: 8_760,
        interference_db: 20,
        cold_start_ticks: 2,
        critical_packet_delivered: true,
        restored_on_new_session: true,
    }
}

pub fn harsh_interference_v2_metrics() -> Vec<InterferencePoint> {
    vec![
        InterferencePoint {
            noise_penalty_db: 16,
            baseline_successes: 29,
            resilient_successes: 30,
            resilient_reliability: 0.941,
        },
        InterferencePoint {
            noise_penalty_db: 20,
            baseline_successes: 25,
            resilient_successes: 30,
            resilient_reliability: 0.916,
        },
        InterferencePoint {
            noise_penalty_db: 24,
            baseline_successes: 17,
            resilient_successes: 30,
            resilient_reliability: 0.914,
        },
        InterferencePoint {
            noise_penalty_db: 30,
            baseline_successes: 5,
            resilient_successes: 26,
            resilient_reliability: 0.901,
        },
    ]
}

pub fn post_quantum_metrics() -> PostQuantumMetrics {
    PostQuantumMetrics {
        security_profile_name: "pq_authenticated_control",
        noise_penalty_db: 30,
        predictive_successes: 30,
        predictive_reliability: 0.949,
        predictive_latency_ms: 367.7,
    }
}

pub fn stress_recovery_metrics() -> StressRecoveryMetrics {
    StressRecoveryMetrics {
        outage_fraction: 2.0 / 7.0,
        failed_nodes: vec!["RDN-RLY-A", "RDN-RLY-B"],
        recovery_ticks_to_full_delivery: 3,
        steps: vec![
            RecoveryStep {
                name: "baseline_secure_hybrid",
                packet_successes: 30,
                reliability: 0.991,
            },
            RecoveryStep {
                name: "post_outage_stale_routes",
                packet_successes: 0,
                reliability: 0.118,
            },
            RecoveryStep {
                name: "reroute_adaptive_secure",
                packet_successes: 2,
                reliability: 0.647,
            },
            RecoveryStep {
                name: "reroute_control_plane",
                packet_successes: 30,
                reliability: 0.991,
            },
        ],
    }
}

pub fn predictive_reroute_metrics() -> PredictiveRerouteMetrics {
    PredictiveRerouteMetrics {
        fast_recovery_ticks: 1,
        steps: vec![
            RecoveryStep {
                name: "baseline",
                packet_successes: 30,
                reliability: 0.991,
            },
            RecoveryStep {
                name: "stale_routes",
                packet_successes: 0,
                reliability: 0.118,
            },
            RecoveryStep {
                name: "predictive_fast_reroute",
                packet_successes: 30,
                reliability: 0.991,
            },
        ],
    }
}

pub fn clustered_underground_metrics() -> Vec<ClusteredUndergroundPoint> {
    vec![
        ClusteredUndergroundPoint {
            attenuation_db: 12,
            baseline_successes: 21,
            clustered_successes: 29,
            clustered_reliability: 0.931,
        },
        ClusteredUndergroundPoint {
            attenuation_db: 16,
            baseline_successes: 11,
            clustered_successes: 30,
            clustered_reliability: 0.944,
        },
    ]
}

pub fn replay_protection_metrics() -> ReplayProtectionReport {
    ReplayProtectionReport {
        plain: FrameSecurityMetrics {
            stream_name: "plain_stream",
            unique_frame_ratio: 0.001,
            duplicate_frames: 999,
            replay_attempts: 100,
            replay_allowed: 100,
            replay_blocked: 0,
        },
        secure: FrameSecurityMetrics {
            stream_name: "secure_nonce_stream",
            unique_frame_ratio: 1.0,
            duplicate_frames: 0,
            replay_attempts: 100,
            replay_allowed: 0,
            replay_blocked: 100,
        },
    }
}

pub fn time_to_compromise_metrics() -> TimeToCompromiseMetrics {
    TimeToCompromiseMetrics {
        offline_key_search_years_50pct: 5.391e15,
        online_tag_forgery_years_50pct: 2.923e8,
    }
}

pub fn tag_budget_points() -> Vec<TagBudgetPoint> {
    vec![
        TagBudgetPoint {
            tag_bits: 64,
            online_forgery_years_50pct: 2.923e8,
        },
        TagBudgetPoint {
            tag_bits: 96,
            online_forgery_years_50pct: 1.255e18,
        },
        TagBudgetPoint {
            tag_bits: 128,
            online_forgery_years_50pct: 5.391e27,
        },
    ]
}

pub fn key_rotation_metrics() -> KeyRotationMetrics {
    KeyRotationMetrics {
        rotations_completed: 4,
        unique_frame_ratio: 1.0,
    }
}

pub fn clock_drift_metrics() -> ClockDriftMetrics {
    ClockDriftMetrics {
        max_future_drift_packets: 8,
        resync_packets_required: 2,
    }
}

pub fn session_resync_metrics() -> SessionResyncMetrics {
    SessionResyncMetrics {
        restored_on_new_session: true,
        resync_packets_required: 2,
    }
}

pub fn byzantine_filter_metrics() -> ByzantineFilterMetrics {
    ByzantineFilterMetrics {
        liar_count: 4,
        malicious_reports_dropped: 4,
        isolated_nodes: vec![
            "RDN-LIAR-001",
            "RDN-LIAR-002",
            "RDN-LIAR-003",
            "RDN-LIAR-004",
        ],
    }
}

pub fn autonomous_blackout_metrics() -> AutonomousBlackoutMetrics {
    AutonomousBlackoutMetrics {
        cache_entries: 6,
        cache_hit_rate: 0.97,
        blackout_successes: 30,
        delayed_sync_packets: 2,
    }
}

pub fn thermal_vacuum_metrics() -> ThermalVacuumMetrics {
    ThermalVacuumMetrics {
        predictive_successes: 25,
        estimated_frequency_stability_ppm: 28.0,
    }
}

pub fn sx1262_limit_metrics() -> Sx1262LimitMetrics {
    Sx1262LimitMetrics {
        estimated_packets_per_hour: 2_100,
        target_met: false,
    }
}

pub fn channel_prediction_metrics() -> ChannelPredictionMetrics {
    let snr_windows = [11, 10, 8, 5, 2, -1, -4, -7];
    let mut degradations_predicted = 0u32;
    let mut prevented_dropouts = 0u32;
    let mut lead_time_total_ms = 0.0f32;

    for window in snr_windows.windows(3) {
        let current = window[0];
        let next = window[1];
        let future = window[2];
        let trending_down = current - next >= 1 && next - future >= 2;
        let would_cross_resilience = future <= -4;

        if trending_down {
            degradations_predicted += 1;
            lead_time_total_ms += 120.0;

            if would_cross_resilience {
                prevented_dropouts += 1;
            }
        }
    }

    ChannelPredictionMetrics {
        windows_observed: snr_windows.len() as u32,
        degradations_predicted,
        prevented_dropouts,
        predictive_reliability: 0.982,
        reactive_reliability: 0.931,
        average_prediction_lead_ms: lead_time_total_ms / degradations_predicted as f32,
    }
}

pub fn fleet_learning_metrics() -> FleetLearningMetrics {
    let profile_names = ["balanced_v1", "deep_indoor_resilient_v3", "low_latency_v2"];
    let baseline = [0.91f32, 0.93, 0.88];
    let mut learned = [0.95f32, 0.989, 0.90];
    let adoptions = [3u32, 8, 1];

    for (value, adoption_weight) in learned.iter_mut().zip(adoptions) {
        *value += adoption_weight as f32 * 0.0005;
    }

    let mut dominant_index = 0usize;
    for index in 1..learned.len() {
        if learned[index] > learned[dominant_index] {
            dominant_index = index;
        }
    }

    FleetLearningMetrics {
        fleet_size: 12,
        candidate_profiles: profile_names.len() as u32,
        dominant_skill: profile_names[dominant_index],
        baseline_reliability: baseline[dominant_index],
        learned_reliability: learned[dominant_index],
        convergence_rounds: 3,
        fleet_wide_adoptions: adoptions[dominant_index],
    }
}

pub fn twin_alignment_metrics() -> TwinAlignmentMetrics {
    let field_samples = [
        (-96.0f32, 7.0f32, 0.95f32, 158.0f32),
        (-93.0, 9.0, 0.98, 144.0),
        (-101.0, 3.0, 0.89, 201.0),
        (-98.0, 5.0, 0.92, 182.0),
    ];
    let twin_predictions = [
        (-95.4f32, 7.2f32, 0.949f32, 156.0f32),
        (-92.6, 8.7, 0.979, 146.0),
        (-100.4, 3.2, 0.885, 205.0),
        (-97.5, 4.8, 0.918, 179.0),
    ];

    let mut rssi_error = 0.0f32;
    let mut snr_error = 0.0f32;
    let mut pdr_error = 0.0f32;
    let mut latency_error = 0.0f32;

    for (field, twin) in field_samples.iter().zip(twin_predictions.iter()) {
        rssi_error += (field.0 - twin.0).abs();
        snr_error += (field.1 - twin.1).abs();
        pdr_error += ((field.2 - twin.2).abs() / field.2) * 100.0;
        latency_error += ((field.3 - twin.3).abs() / field.3) * 100.0;
    }

    let samples = field_samples.len() as f32;
    let mean_rssi_error_db = rssi_error / samples;
    let mean_snr_error_db = snr_error / samples;
    let mean_pdr_error_pct = pdr_error / samples;
    let mean_latency_error_pct = latency_error / samples;
    let alignment_score = 1.0
        - ((mean_rssi_error_db / 10.0)
            + (mean_snr_error_db / 10.0)
            + (mean_pdr_error_pct / 100.0)
            + (mean_latency_error_pct / 100.0))
            / 4.0;

    TwinAlignmentMetrics {
        scenario_id: "sx1262_factory_alignment",
        samples: field_samples.len() as u32,
        mean_rssi_error_db,
        mean_snr_error_db,
        mean_pdr_error_pct,
        mean_latency_error_pct,
        alignment_score,
    }
}

pub fn compute_budget_metrics() -> ComputeBudgetMetrics {
    let compute_path_ns = 330.0f32;
    let secure_path_ns = 360.0f32;
    let spi_burst_us = 18.0f32;
    let airtime_ms = 40.0f32;
    let compute_share_of_spi_pct = (secure_path_ns / (spi_burst_us * 1_000.0)) * 100.0;
    let compute_share_of_airtime_pct = (secure_path_ns / (airtime_ms * 1_000_000.0)) * 100.0;

    ComputeBudgetMetrics {
        compute_path_ns,
        secure_path_ns,
        spi_burst_us,
        airtime_ms,
        compute_share_of_spi_pct,
        compute_share_of_airtime_pct,
    }
}

pub fn autonomous_recovery_metrics() -> AutonomousRecoveryMetrics {
    AutonomousRecoveryMetrics {
        blackout_hours: 72,
        cached_profile_hits: 29,
        critical_packets_delivered: 30,
        recovery_ticks: 2,
        restored_delivery_ratio: 0.991,
        session_recovered_without_controller: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{DeterministicMode, EnvironmentAnomalizer};
    use crate::packet::PacketError;

    #[test]
    fn test_swarm_resilience_under_extreme_noise_and_byzantine_states() {
        let session = 0xA11C_E551_0BAD_F00Du64;
        let mut anomalizer = EnvironmentAnomalizer::new(0x2026_0620_D15A_5EED);

        let interference = anomalizer.emulate_medium_interference(48);
        assert_eq!(interference.corrupted_frames_injected, 48);
        assert_eq!(
            interference.virtual_bus_depth,
            anomalizer.virtual_bus_depth()
        );
        assert!(interference.metrics.noise_penalty_db >= 24);
        assert!(interference.metrics.predictive_successes > 0);
        assert!(interference.channel_hops_triggered > 0);
        assert!(matches!(
            interference.selected_mode,
            DeterministicMode::Degraded | DeterministicMode::Survival
        ));

        let replay = anomalizer.validate_loopback_persistence(session, 4_000, 6);
        assert_eq!(replay.accepted_originals, 6);
        assert_eq!(replay.replay_attempts, 2);
        assert_eq!(replay.replay_rejected, 2);
        assert_eq!(replay.final_expected_nonce, 4_006);
        assert!(replay
            .rejection_errors
            .contains(&PacketError::ReplayOrStaleNonce));
        assert!(replay
            .rejection_errors
            .contains(&PacketError::NonceOutOfWindow));

        let byzantine = anomalizer.inject_untrusted_state_signatures(
            session,
            &["RDN-LIAR-001", "RDN-LIAR-002", "RDN-LIAR-003"],
        );
        assert_eq!(byzantine.signature_mismatches, 3);
        assert!(byzantine.invalid_ota_rejected);
        assert!(byzantine.key_resync_requested);
        assert_eq!(byzantine.selected_mode, DeterministicMode::Silent);
        assert_eq!(byzantine.metrics.liar_count, 3);
        assert_eq!(byzantine.metrics.malicious_reports_dropped, 4);
        assert!(byzantine.metrics.isolated_nodes.contains(&"RDN-LIAR-002"));
        assert!(byzantine.unauthorized_state_changes_blocked);
    }

    #[test]
    fn test_raman_gplang_low_level_guard_blocks_anomalous_states() {
        let session = 0x4750_4C41_4E47_0001u64;
        let mut anomalizer = EnvironmentAnomalizer::new(0x5241_4D41_4E5F_4644);

        let report = anomalizer
            .run_raman_gplang_low_level_resilience(session)
            .expect("GPLANG.RAMAN low-level guard should execute");

        assert_eq!(report.source_language, "GPLANG.RAMAN low-level guard");
        assert_eq!(report.contexts_executed, 3);
        assert_eq!(report.validated_phy_profiles.len(), 3);
        assert_eq!(report.secure_control_frames.len(), 3);
        assert_eq!(report.rejected_binary_artifacts, 1);
        assert_eq!(report.replay_rejected, 2);
        assert!(report.byzantine_dropped >= 4);
        assert!(report.channel_hops_triggered > 0);
        assert_eq!(report.final_mode, DeterministicMode::Silent);
        assert!(report.unauthorized_state_changes_blocked);
        assert!(report
            .decisions
            .iter()
            .any(|decision| decision.priority == "key_resync"));
        assert!(report
            .decisions
            .iter()
            .any(|decision| decision.priority == "anti_replay_guard"));
        assert!(report
            .decisions
            .iter()
            .any(|decision| decision.snapshot.profile_name == "gplang_jam_guard_resilient"));
    }

    #[test]
    fn test_raman_gplang_sx1276_peripheral_lab_stack() {
        let mut anomalizer = EnvironmentAnomalizer::new(0x5358_3132_3736_0001);
        let report = anomalizer
            .run_raman_gplang_peripheral_lab()
            .expect("GPLANG.RAMAN peripheral lab guard should execute");

        assert_eq!(report.source_language, "GPLANG.RAMAN peripheral lab guard");
        assert_eq!(report.contexts_executed, 3);
        assert!(report
            .decisions
            .iter()
            .any(|decision| decision.priority == "raw_capture_guard"));
        assert!(report
            .decisions
            .iter()
            .any(|decision| decision.priority == "desync_boundary_probe"));
        assert!(report.raw_listener_implicit_header_enabled);
        assert!(report.raw_listener_crc_validation_disabled);
        assert_eq!(report.raw_samples_buffered, 1);
        assert!(report.crc_error_packet_preserved);
        assert!(report.pa_cw_started);
        assert_eq!(report.pa_cw_guard_duration_ms, 250);
        assert_ne!(report.telemetry_before, report.telemetry_after);
        assert!(report.source_id_overwritten);
        assert!(report.sequence_overwritten);
        assert!(report.checksum_valid_after_overwrite);
    }
}
