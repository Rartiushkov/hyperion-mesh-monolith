use std::path::PathBuf;

use radnet_morphic_kernel::{load_raman_artifact, run_portable_pipeline, RamanRuntimeContext};

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

#[test]
fn same_artifact_produces_same_runtime_verdict_across_three_backends() {
    let artifact = load_raman_artifact(fixture_path("contracts/raman_policy_v1.json"))
        .expect("artifact should load");
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

    let backends = [
        "chips/espressif/esp32c3_sx1262",
        "chips/st/stm32wle5_sx126x",
        "chips/nordic/nrf52840_sx1262",
    ];

    let reports = backends
        .iter()
        .map(|pack| {
            run_portable_pipeline(&artifact, &context, "portable-e2e", fixture_path(pack))
                .expect("portable pipeline should succeed")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        reports[0].decision.snapshot.profile_name,
        reports[1].decision.snapshot.profile_name
    );
    assert_eq!(
        reports[1].decision.snapshot.profile_name,
        reports[2].decision.snapshot.profile_name
    );
    assert_eq!(reports[0].decision.priority, reports[1].decision.priority);
    assert_eq!(reports[1].decision.priority, reports[2].decision.priority);
    assert!(!reports[0].writes.is_empty());
    assert!(!reports[1].writes.is_empty());
    assert!(!reports[2].writes.is_empty());
}
