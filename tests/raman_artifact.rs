use radnet_morphic_kernel::{
    candidate_rule_count, encode_raman_artifact_binary, parse_raman_artifact,
    parse_raman_artifact_binary, validate_raman_artifact, Context, RamanExecutorState,
    RamanMirrorExecutor, RamanRuntimeArtifact, RamanRuntimeContext,
};
use serde::Deserialize;

const DEFAULT_ARTIFACT: &str = include_str!("../contracts/raman_policy_v1.json");
const PARITY_VECTORS: &str = include_str!("../contracts/raman_parity_vectors_v1.json");

#[test]
fn parses_default_raman_artifact_from_contract_file() {
    let artifact = parse_raman_artifact(DEFAULT_ARTIFACT).expect("artifact should parse");

    assert_eq!(artifact.metadata.artifact_version, "raman-artifact/v1");
    assert_eq!(artifact.metadata.runtime, "RamanExecutor v1");
    assert_eq!(artifact.metadata.target, "esp32c3_sx1262");
    assert_eq!(artifact.ir.total_rules, artifact.ir.rules.len());
    assert_eq!(artifact.executable.rules.len(), artifact.ir.total_rules);
    assert_eq!(artifact.device_abi.version, "raman-device-abi/v1");
    assert_eq!(artifact.device_abi.rules.len(), artifact.ir.total_rules);
}

#[test]
fn validates_default_raman_artifact_for_rust_runtime_bridge() {
    let artifact: RamanRuntimeArtifact =
        parse_raman_artifact(DEFAULT_ARTIFACT).expect("artifact should parse");
    let validation = validate_raman_artifact(&artifact).expect("artifact should validate");

    assert_eq!(validation.total_rules, 22);
    assert_eq!(validation.target, "esp32c3_sx1262");
    assert_eq!(
        validation.indexed_fields,
        vec![
            "traffic".to_string(),
            "hardware".to_string(),
            "scenario".to_string()
        ]
    );
    assert!(validation.traffic_candidate_rules >= 4);
    assert_eq!(artifact.device_abi.rules.len(), artifact.ir.total_rules);
}

#[test]
fn counts_candidate_rules_from_exact_match_filters() {
    let artifact = parse_raman_artifact(DEFAULT_ARTIFACT).expect("artifact should parse");

    let truth_candidates = candidate_rule_count(
        &artifact,
        Some("truth_critical"),
        Some("sx1262_lab"),
        Some("industrial_shift"),
    );
    let bulk_relay_candidates = candidate_rule_count(
        &artifact,
        Some("bulk_observable"),
        Some("sx1276_relay"),
        Some("industrial_shift"),
    );
    let voice_candidates = candidate_rule_count(
        &artifact,
        Some("voice_live"),
        Some("generic"),
        Some("default"),
    );

    assert_eq!(truth_candidates, 7);
    assert_eq!(bulk_relay_candidates, 3);
    assert_eq!(voice_candidates, 2);
}

#[test]
fn loads_binary_raman_artifact_with_same_contract() {
    let json_artifact = parse_raman_artifact(DEFAULT_ARTIFACT).expect("json artifact should parse");
    let raw = encode_raman_artifact_binary(&json_artifact);
    let artifact = parse_raman_artifact_binary(&raw).expect("binary artifact should parse");

    assert_eq!(artifact.metadata.artifact_version, "raman-artifact/v1");
    assert_eq!(artifact.metadata.runtime, "RamanExecutor v1");
    assert_eq!(artifact.ir.total_rules, 22);
    assert_eq!(artifact.executable.rules.len(), artifact.ir.total_rules);
    assert_eq!(artifact.device_abi.rules.len(), artifact.ir.total_rules);
}

#[test]
fn rust_mirror_executor_resolves_truth_window_from_artifact() {
    let artifact = parse_raman_artifact(DEFAULT_ARTIFACT).expect("artifact should parse");
    let executor = RamanMirrorExecutor::from_artifact(&artifact).expect("executor should build");
    let mut state = RamanExecutorState::new();
    let result = executor
        .execute(
            "truth-stream",
            &RamanRuntimeContext {
                minute: 30,
                traffic_class: "truth_critical".to_string(),
                hardware: "sx1262_lab".to_string(),
                scenario: "industrial_shift".to_string(),
                noise_floor_dbm: -110,
                snr_db: -1.0,
                density: 60,
                latency_budget_ms: 120,
                battery_mv: 3480,
                link_margin_db: 3.0,
                confidence: 0.70,
                predicted: "degrading".to_string(),
                drift_db: 1.0,
                renegotiation_needed: true,
                rx_pdr: 0.30,
                error_rate: 0.08,
                path_stability: 0.42,
                memory_confidence: 0.55,
                worst_hop_reliability: 0.33,
            },
            &mut state,
        )
        .expect("execution should succeed");

    assert_eq!(result.snapshot.profile_name, "ril_truth_guard");
    assert_eq!(result.priority, "critical");
    assert_eq!(
        result.snapshot.transport_priority.as_deref(),
        Some("critical")
    );
    assert_eq!(
        result.snapshot.ris.as_ref().map(|ris| ris.mode.as_str()),
        Some("ris_assisted")
    );
    assert_eq!(
        result
            .snapshot
            .verify
            .as_ref()
            .map(|verify| verify.metric.as_str()),
        Some("path_stability")
    );
}

#[test]
fn rust_mirror_executor_reuses_temporal_hold_state() {
    let artifact = parse_raman_artifact(
        r#"{
          "metadata": {
            "artifact_version": "raman-artifact/v1",
            "source_kind": "rpl",
            "source_name": "hold_test.rpl",
            "target": "esp32c3_sx1262",
            "runtime": "RamanExecutor v1"
          },
          "compiled_rpl": "priority 340 when traffic == bulk_observable and minute == 0 -> phy(name=held_bulk, bandwidth_khz=125, spreading_factor=11, coding_rate_denominator=8, tx_power_dbm=15, preamble_symbols=12)\npriority 339 when traffic == bulk_observable and minute == 0 -> hold(ticks=1)",
          "ir": {
            "total_rules": 2,
            "indexed_fields": ["traffic", "hardware", "scenario"],
            "rules": [
              { "priority": 340, "order": 0, "action_kind": "phy", "exact_filters": [["traffic", "bulk_observable"]] },
              { "priority": 339, "order": 1, "action_kind": "hold", "exact_filters": [["traffic", "bulk_observable"]] }
            ]
          }
        }"#,
    )
    .expect("artifact should parse");
    let executor = RamanMirrorExecutor::from_artifact(&artifact).expect("executor should build");
    let mut state = RamanExecutorState::new();
    let first = executor
        .execute(
            "bulk-stream",
            &RamanRuntimeContext::from_core(
                &Context {
                    noise_floor_dbm: -111,
                    snr_db: 1,
                    battery_mv: 3500,
                    latency_budget_ms: 120,
                    link_margin_db: 4,
                },
                0,
                "bulk_observable",
                "generic",
                "industrial_shift",
                58,
            ),
            &mut state,
        )
        .expect("first execution should succeed");
    let second = executor
        .execute(
            "bulk-stream",
            &RamanRuntimeContext::from_core(
                &Context {
                    noise_floor_dbm: -111,
                    snr_db: 1,
                    battery_mv: 3500,
                    latency_budget_ms: 120,
                    link_margin_db: 4,
                },
                15,
                "bulk_observable",
                "generic",
                "industrial_shift",
                58,
            ),
            &mut state,
        )
        .expect("second execution should succeed");

    assert_eq!(first.snapshot.profile_name, "held_bulk");
    assert_eq!(first.temporal_mode.as_deref(), Some("hold"));
    assert!(!first.temporal_reused);
    assert_eq!(second.snapshot.profile_name, "held_bulk");
    assert_eq!(second.temporal_mode.as_deref(), Some("hold"));
    assert!(second.temporal_reused);
}

#[derive(Debug, Deserialize)]
struct ParityFixture {
    artifact_path: String,
    cases: Vec<ParityCase>,
}

#[derive(Debug, Deserialize)]
struct ParityCase {
    name: String,
    stream_id: String,
    context: ParityContext,
    expected: ParityExpected,
}

#[derive(Debug, Deserialize)]
struct ParityContext {
    minute: i32,
    traffic_class: String,
    hardware: String,
    scenario: String,
    noise_floor_dbm: i16,
    snr_db: f32,
    density: i32,
    latency_budget_ms: i32,
    battery_mv: i32,
    link_margin_db: f32,
    confidence: f32,
    predicted: String,
    drift_db: f32,
    renegotiation_needed: bool,
    rx_pdr: f32,
    error_rate: f32,
    path_stability: f32,
    memory_confidence: f32,
    worst_hop_reliability: f32,
}

#[derive(Debug, Deserialize)]
struct ParityExpected {
    profile_name: String,
    priority: String,
    transport_priority: Option<String>,
    ris_mode: Option<String>,
    verify_metric: Option<String>,
    temporal_mode: Option<String>,
    temporal_reused: bool,
}

#[test]
fn rust_mirror_executor_matches_shared_parity_vectors() {
    let fixture: ParityFixture =
        serde_json::from_str(PARITY_VECTORS).expect("fixture should parse");
    let artifact_raw =
        std::fs::read_to_string(&fixture.artifact_path).expect("artifact path should load");
    let artifact = parse_raman_artifact(&artifact_raw).expect("artifact should parse");
    let executor = RamanMirrorExecutor::from_artifact(&artifact).expect("executor should build");

    for case in fixture.cases {
        let mut state = RamanExecutorState::new();
        let result = executor
            .execute(
                &case.stream_id,
                &RamanRuntimeContext {
                    minute: case.context.minute,
                    traffic_class: case.context.traffic_class,
                    hardware: case.context.hardware,
                    scenario: case.context.scenario,
                    noise_floor_dbm: case.context.noise_floor_dbm,
                    snr_db: case.context.snr_db,
                    density: case.context.density,
                    latency_budget_ms: case.context.latency_budget_ms,
                    battery_mv: case.context.battery_mv,
                    link_margin_db: case.context.link_margin_db,
                    confidence: case.context.confidence,
                    predicted: case.context.predicted,
                    drift_db: case.context.drift_db,
                    renegotiation_needed: case.context.renegotiation_needed,
                    rx_pdr: case.context.rx_pdr,
                    error_rate: case.context.error_rate,
                    path_stability: case.context.path_stability,
                    memory_confidence: case.context.memory_confidence,
                    worst_hop_reliability: case.context.worst_hop_reliability,
                },
                &mut state,
            )
            .unwrap_or_else(|_| panic!("parity case failed: {}", case.name));

        assert_eq!(
            result.snapshot.profile_name, case.expected.profile_name,
            "{}",
            case.name
        );
        assert_eq!(result.priority, case.expected.priority, "{}", case.name);
        assert_eq!(
            result.snapshot.transport_priority, case.expected.transport_priority,
            "{}",
            case.name
        );
        assert_eq!(
            result.snapshot.ris.as_ref().map(|ris| ris.mode.clone()),
            case.expected.ris_mode,
            "{}",
            case.name
        );
        assert_eq!(
            result
                .snapshot
                .verify
                .as_ref()
                .map(|verify| verify.metric.clone()),
            case.expected.verify_metric,
            "{}",
            case.name
        );
        assert_eq!(
            result.temporal_mode, case.expected.temporal_mode,
            "{}",
            case.name
        );
        assert_eq!(
            result.temporal_reused, case.expected.temporal_reused,
            "{}",
            case.name
        );
    }
}
