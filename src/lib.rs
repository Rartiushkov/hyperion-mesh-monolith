#![cfg_attr(not(feature = "std"), no_std)]

pub mod assurance;
#[cfg(feature = "std")]
pub mod blockchain;
pub mod boards;
pub mod channel_ensemble;
pub mod context;
#[cfg(feature = "std")]
pub mod db;
pub mod drivers;
pub mod error;
#[cfg(feature = "std")]
pub mod ffi;
pub mod hal;
pub mod kernel;
pub mod packet;
pub mod phy;
#[cfg(feature = "std")]
pub mod portability;
pub mod power;
pub mod radio;
#[cfg(feature = "std")]
pub mod raman;
#[cfg(feature = "std")]
pub mod raman_abi;
#[cfg(feature = "std")]
pub mod runtime;
#[cfg(feature = "std")]
pub mod sim;
pub mod telemetry;

pub use assurance::{
    certify_scenarios, evaluate_seu, parity_gate, redundant_vote, verify_secure_chain,
    DeterministicMode, FaultClass, FaultEvent, FaultSymptom, FdirEngine, FdirReport,
    HealthGovernor, HealthInput, ParityGateReport, ParityThresholds, RecoveryAction,
    RedundantRuntimeVerdict, ResidualRisk, RuntimeDecisionDigest, RuntimeTelemetry, SafetyCheck,
    SafetyInvariants, SafetyViolation, ScenarioCertificationReport, ScenarioCertificationRow,
    SecureMissionInput, SecureMissionReport, SeuGuardReport, SeuSlotState, TemporalContracts,
    TemporalReport, TemporalSample, TwinHardwareMetrics,
};
pub use boards::{Esp32C3BoardConfig, Esp32C3RadioPins, Esp32C3Sx1262Board};
pub use channel_ensemble::{
    BleCrossLayer, BruFusion, BruFusionResult, ChannelEnsemble, ChannelMeasurement,
    ChannelTechnology, EnsembleChannel, FullBandHopPlan, WifiCrossLayer,
};
pub use context::Context;
pub use drivers::{
    FieldLogEntry, LogSink, MemoryLogSink, MockSx1262Bus, MockSx1276Spi, PaThermalCwConfig,
    PaThermalPlan, RawListenerConfig, RawPacketSample, RegisterBus, Sx1262Driver, Sx1276Driver,
    Sx1276RawPacketRing, Sx1276SpiBus,
};
pub use error::{KernelError, ValidationError};
#[cfg(feature = "std")]
pub use ffi::{
    raman_process_signal, raman_process_signal_binary, raman_runtime_free,
    raman_runtime_new_from_binary,
};
pub use hal::{
    DelayUs, DigitalInput, DigitalOutput, HalSx1262Bus, SpiBus, SX1262_SET_SLEEP_OPCODE,
    SX1262_SET_STANDBY_OPCODE,
};
#[cfg(feature = "embedded-hal-adapter")]
pub use hal::{EmbeddedHalDelay, EmbeddedHalInputPin, EmbeddedHalOutputPin, EmbeddedHalSpiBus};
pub use kernel::{MorphicKernel, ProfilePlanner};
pub use packet::{accept_frame, resync_window, PacketError, ReplayWindow, SecureFrame32};
pub use phy::{
    Bandwidth, CodingRate, HeaderMode, Modulation, PhyProfile, ProfileDiff, ProfilePreset,
    SpreadingFactor,
};
#[cfg(feature = "std")]
pub use portability::{
    compile_rhml_command_plan, derive_artifact_requirements, load_chip_pack,
    negotiate_artifact_with_capability, run_portable_pipeline, PortablePipelineReport,
    RamanAbiNegotiationReport, RamanArtifactRequirements, RamanCapabilityDescriptor, RamanChipPack,
    RamanRhmlProgram,
};
pub use power::AdaptivePowerManager;
pub use radio::{MockRadio, RadioDriver, RegisterWrite, WritePlan, MAX_REGISTER_WRITES};
#[cfg(feature = "std")]
pub use raman::{
    candidate_rule_count, encode_raman_artifact_binary, load_raman_artifact,
    load_raman_artifact_binary, parse_raman_artifact, parse_raman_artifact_binary,
    validate_raman_artifact, RamanArtifactError, RamanArtifactMetadata, RamanArtifactValidation,
    RamanDeviceAbiAction, RamanDeviceAbiPredicate, RamanDeviceAbiProgram, RamanDeviceAbiRule,
    RamanExecutableAction, RamanExecutablePredicate, RamanExecutableProgram, RamanExecutableRule,
    RamanExecutorState, RamanMirrorExecutor, RamanProgramIr, RamanRuleIr, RamanRuntimeArtifact,
    RamanRuntimeContext, RamanRuntimeResult, RamanRuntimeRis, RamanRuntimeSnapshot,
    RamanRuntimeVerify,
};
#[cfg(feature = "std")]
pub use raman_abi::{
    RamanArtifactSlotHeader, RamanCSignalInput, RamanCSignalOutput,
    RAMAN_ARTIFACT_FLAG_FORCE_UPDATE, RAMAN_ARTIFACT_SLOT_MAGIC, RAMAN_DEVICE_ABI_VERSION_MAJOR,
    RAMAN_DEVICE_ABI_VERSION_MINOR,
};
pub use raman_core::{
    execute as execute_raman_core, symbol_id as raman_symbol_id,
    symbol_id_const as raman_symbol_id_const, CompareOp as RamanCoreCompareOp,
    CoreAction as RamanCoreAction, CoreContext as RamanCoreContext,
    CoreDecision as RamanCoreDecision, CoreExecutorState as RamanCoreExecutorState,
    CoreProgram as RamanCoreProgram, CoreRule as RamanCoreRule, FieldId as RamanCoreFieldId,
    Predicate as RamanCorePredicate, SymbolId as RamanCoreSymbolId,
};
#[cfg(feature = "std")]
pub use runtime::{
    NoopRamanPlatform, RamanClock, RamanControlPlane, RamanHostReport, RamanRuntimeHost,
};
#[cfg(feature = "std")]
pub use sim::{
    autonomous_blackout_metrics, autonomous_recovery_metrics, byzantine_filter_metrics,
    byzantine_threshold_metrics, channel_prediction_metrics, clock_drift_metrics,
    clustered_underground_metrics, cold_start_metrics, compute_budget_metrics,
    doppler_resilience_metrics, fleet_learning_metrics, harsh_interference_v2_metrics,
    key_rotation_metrics, post_quantum_metrics, predictive_reroute_metrics,
    radiation_jamming_metrics, replay_protection_metrics, session_resync_metrics,
    stress_recovery_metrics, sx1262_limit_metrics, tag_budget_points, thermal_vacuum_metrics,
    time_to_compromise_metrics, twin_alignment_metrics, voice_stress_report,
    AutonomousBlackoutMetrics, AutonomousRecoveryMetrics, ByzantineFilterMetrics,
    ByzantineInjectionReport, ByzantineThresholdMetrics, ChannelPredictionMetrics,
    ClockDriftMetrics, ClusteredUndergroundPoint, ColdStartMetrics, ComputeBudgetMetrics,
    DopplerMetrics, EnvironmentAnomalizer, FleetLearningMetrics, FrameSecurityMetrics,
    InterferencePoint, KeyRotationMetrics, LoopbackPersistenceReport, MediumInterferenceReport,
    PostQuantumMetrics, PredictiveRerouteMetrics, RadiationJammingMetrics,
    RamanGplangLowLevelReport, RamanGplangPeripheralLabReport, RecoveryStep,
    ReplayProtectionReport, SessionResyncMetrics, StressRecoveryMetrics, Sx1262LimitMetrics,
    TagBudgetPoint, ThermalVacuumMetrics, TimeToCompromiseMetrics, TwinAlignmentMetrics,
    VoiceStrategyMetrics, VoiceStressReport,
};
pub use telemetry::{
    TelemetryFrame, TelemetryFrameFields, TELEMETRY_FRAME_LEN, TELEMETRY_MAGIC,
    TELEMETRY_TERMINATOR,
};
