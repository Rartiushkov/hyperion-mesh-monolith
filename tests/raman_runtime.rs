use std::ffi::CString;

use radnet_morphic_kernel::{
    encode_raman_artifact_binary, parse_raman_artifact, raman_process_signal_binary,
    NoopRamanPlatform, RamanCSignalInput, RamanCSignalOutput, RamanRuntimeHost,
};

const DEFAULT_ARTIFACT: &str = include_str!("../contracts/raman_policy_v1.json");

#[test]
fn runtime_host_applies_signal_to_generic_platform() {
    let artifact = parse_raman_artifact(DEFAULT_ARTIFACT).expect("artifact should parse");
    let platform = NoopRamanPlatform::default();
    let mut host = RamanRuntimeHost::from_artifact(platform, &artifact).expect("host should build");

    let report = host
        .process_signal(
            "ffi-stream",
            &radnet_morphic_kernel::RamanRuntimeContext {
                minute: 30,
                traffic_class: "truth_critical".into(),
                hardware: "sx1262_lab".into(),
                scenario: "industrial_shift".into(),
                noise_floor_dbm: -110,
                snr_db: -1.0,
                density: 60,
                latency_budget_ms: 120,
                battery_mv: 3480,
                link_margin_db: 3.0,
                confidence: 0.70,
                predicted: "degrading".into(),
                drift_db: 1.0,
                renegotiation_needed: true,
                rx_pdr: 0.30,
                error_rate: 0.08,
                path_stability: 0.42,
                memory_confidence: 0.55,
                worst_hop_reliability: 0.33,
            },
        )
        .expect("signal should apply");

    assert_eq!(report.decision.snapshot.profile_name, "ril_truth_guard");
    assert_eq!(
        host.platform().last_transport_priority.as_deref(),
        Some("critical")
    );
    assert_eq!(
        host.platform().last_ris_mode.as_deref(),
        Some("ris_assisted")
    );
}

#[test]
fn c_abi_processes_signal_in_one_call() {
    let artifact = parse_raman_artifact(DEFAULT_ARTIFACT).expect("artifact should parse");
    let artifact_raw = encode_raman_artifact_binary(&artifact);
    let mut output = RamanCSignalOutput::default();

    let stream_id = CString::new("ffi-stream").unwrap();
    let traffic = CString::new("truth_critical").unwrap();
    let hardware = CString::new("sx1262_lab").unwrap();
    let scenario = CString::new("industrial_shift").unwrap();
    let predicted = CString::new("degrading").unwrap();
    let input = RamanCSignalInput {
        minute: 30,
        noise_floor_dbm: -110,
        snr_db: -1.0,
        density: 60,
        latency_budget_ms: 120,
        battery_mv: 3480,
        link_margin_db: 3.0,
        confidence: 0.70,
        drift_db: 1.0,
        rx_pdr: 0.30,
        error_rate: 0.08,
        path_stability: 0.42,
        memory_confidence: 0.55,
        worst_hop_reliability: 0.33,
        renegotiation_needed: 1,
        traffic_class: traffic.as_ptr(),
        hardware: hardware.as_ptr(),
        scenario: scenario.as_ptr(),
        predicted: predicted.as_ptr(),
        stream_id: stream_id.as_ptr(),
    };

    let status = raman_process_signal_binary(
        artifact_raw.as_ptr(),
        artifact_raw.len(),
        &input,
        &mut output,
    );

    assert_eq!(status, 0);
    assert_eq!(output.bandwidth_khz, 125);
    assert!(output.spreading_factor >= 7);
}
