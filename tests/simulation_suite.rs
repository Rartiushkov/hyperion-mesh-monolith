use pretty_assertions::assert_eq;
use proptest::prelude::*;
use radnet_morphic_kernel::{
    autonomous_blackout_metrics, autonomous_recovery_metrics, byzantine_filter_metrics,
    byzantine_threshold_metrics, channel_prediction_metrics, clock_drift_metrics,
    clustered_underground_metrics, cold_start_metrics, compute_budget_metrics,
    doppler_resilience_metrics, fleet_learning_metrics, harsh_interference_v2_metrics,
    key_rotation_metrics, post_quantum_metrics, predictive_reroute_metrics,
    radiation_jamming_metrics, replay_protection_metrics, session_resync_metrics,
    stress_recovery_metrics, sx1262_limit_metrics, tag_budget_points, thermal_vacuum_metrics,
    time_to_compromise_metrics, twin_alignment_metrics, voice_stress_report, Context, MockRadio,
    MorphicKernel,
};

#[test]
fn voice_stress_meets_live_dialog_budget() {
    let report = voice_stress_report();

    assert_eq!(report.topology_name, "12-node mine voice chain");
    assert_eq!(report.packet_interval_ms, 40);
    assert_eq!(report.payload_bytes, 36);
    assert!(report.coordinated_cluster.average_end_to_end_latency_ms <= 450.0);
    assert!(report.predictive_hybrid.average_end_to_end_latency_ms <= 450.0);
    assert!(report.coordinated_cluster.end_to_end_jitter_ms <= 5.0);
    assert!(report.predictive_hybrid.end_to_end_jitter_ms <= 5.0);
    assert!(report.coordinated_cluster.voice_frame_loss_rate_pct < 3.0);
    assert!(report.predictive_hybrid.voice_frame_loss_rate_pct <= 1.0);
    assert!(report.predictive_hybrid.buffer_underflow_count <= 1);
    assert!(
        report.predictive_hybrid.successful_frames >= report.coordinated_cluster.successful_frames
    );
    assert!(report
        .predictive_hybrid
        .average_per_hop_latency_ms
        .iter()
        .all(|latency| *latency <= 40.0));
    assert!(report
        .coordinated_cluster
        .average_per_hop_latency_ms
        .iter()
        .all(|latency| *latency <= 40.0));
}

#[test]
fn radiation_and_doppler_reports_hold_extreme_thresholds() {
    let radiation = radiation_jamming_metrics();
    let doppler = doppler_resilience_metrics();

    assert_eq!(radiation.noise_penalty_db, 24);
    assert!(radiation.predictive_successes >= 28);
    assert!(radiation.predictive_reliability >= 0.90);

    assert_eq!(doppler.speed_mps, 7_800);
    assert!(doppler.effective_shift_khz >= 100.0);
    assert!(doppler.predictive_successes >= 28);
    assert!(doppler.predictive_latency_ms <= 500.0);
    assert!(doppler.predictive_reliability >= 0.95);
}

#[test]
fn byzantine_threshold_and_filter_hold_for_forty_percent_liars() {
    let threshold_points = byzantine_threshold_metrics();
    let point_40 = threshold_points
        .iter()
        .find(|point| (point.liar_fraction - 0.4).abs() < f32::EPSILON)
        .unwrap();
    let filter = byzantine_filter_metrics();

    assert_eq!(point_40.honest_successes, 30);
    assert_eq!(point_40.liars_isolated, 4);
    assert!(point_40.recovery_ticks <= 3);

    assert!(filter.liar_count >= 2);
    assert_eq!(filter.malicious_reports_dropped, filter.liar_count);
    assert!(!filter.isolated_nodes.is_empty());
}

#[test]
fn cold_start_blackout_and_resync_stay_operational() {
    let cold_start = cold_start_metrics();
    let blackout = autonomous_blackout_metrics();
    let resync = session_resync_metrics();
    let drift = clock_drift_metrics();

    assert!(cold_start.critical_packet_delivered);
    assert!(cold_start.restored_on_new_session);
    assert!(cold_start.cold_start_ticks <= 2);

    assert!(blackout.cache_entries > 0);
    assert!(blackout.cache_hit_rate > 0.9);
    assert!(blackout.blackout_successes >= 20);
    assert!(blackout.delayed_sync_packets >= 1);

    assert!(resync.restored_on_new_session);
    assert!(resync.resync_packets_required >= 1);

    assert_eq!(drift.max_future_drift_packets, 8);
    assert!(drift.resync_packets_required >= 1);
}

#[test]
fn interference_reroute_and_recovery_match_target_envelopes() {
    let interference = harsh_interference_v2_metrics();
    let point_24 = interference
        .iter()
        .find(|point| point.noise_penalty_db == 24)
        .unwrap();
    let reroute = predictive_reroute_metrics();
    let recovery = stress_recovery_metrics();
    let underground = clustered_underground_metrics();
    let deep_point = underground
        .iter()
        .find(|point| point.attenuation_db == 16)
        .unwrap();

    assert_eq!(point_24.resilient_successes, 30);
    assert!(point_24.resilient_successes > point_24.baseline_successes);

    assert_eq!(reroute.fast_recovery_ticks, 1);
    assert_eq!(reroute.steps.first().unwrap().packet_successes, 30);
    assert_eq!(reroute.steps.last().unwrap().packet_successes, 30);

    assert!((recovery.outage_fraction - (2.0 / 7.0)).abs() < 0.001);
    assert_eq!(recovery.steps.first().unwrap().packet_successes, 30);
    assert_eq!(recovery.steps.last().unwrap().packet_successes, 30);
    assert!(recovery.recovery_ticks_to_full_delivery <= 3);

    assert!(deep_point.clustered_successes >= 25);
    assert!(deep_point.clustered_successes > deep_point.baseline_successes);
}

#[test]
fn security_budget_reports_improve_with_nonce_and_tag_length() {
    let replay = replay_protection_metrics();
    let compromise = time_to_compromise_metrics();
    let tag_points = tag_budget_points();
    let pq = post_quantum_metrics();
    let rotation = key_rotation_metrics();

    assert!(replay.secure.unique_frame_ratio > replay.plain.unique_frame_ratio);
    assert_eq!(replay.secure.replay_allowed, 0);
    assert_eq!(replay.secure.replay_blocked, replay.secure.replay_attempts);

    assert!(compromise.offline_key_search_years_50pct > 1.0e12);
    assert!(compromise.online_tag_forgery_years_50pct > 1.0e6);

    assert_eq!(
        tag_points
            .iter()
            .map(|point| point.tag_bits)
            .collect::<Vec<_>>(),
        vec![64, 96, 128]
    );
    assert!(tag_points[0].online_forgery_years_50pct < tag_points[2].online_forgery_years_50pct);

    assert!(rotation.rotations_completed > 0);
    assert_eq!(rotation.unique_frame_ratio, 1.0);

    assert!(pq.predictive_successes >= 26);
    assert!(pq.predictive_reliability >= 0.90);
}

#[test]
fn thermal_and_sx1262_limits_are_reported() {
    let thermal = thermal_vacuum_metrics();
    let limits = sx1262_limit_metrics();

    assert!(thermal.predictive_successes >= 24);
    assert!(thermal.estimated_frequency_stability_ppm < 30.0);

    assert!(limits.estimated_packets_per_hour > 0);
    assert!(!limits.target_met);
}

#[test]
fn prediction_learning_alignment_and_autonomy_hold_scifi_thresholds() {
    let prediction = channel_prediction_metrics();
    let learning = fleet_learning_metrics();
    let alignment = twin_alignment_metrics();
    let budget = compute_budget_metrics();
    let autonomy = autonomous_recovery_metrics();

    assert!(prediction.degradations_predicted >= 2);
    assert!(prediction.prevented_dropouts >= 1);
    assert!(prediction.predictive_reliability > prediction.reactive_reliability);
    assert!(prediction.average_prediction_lead_ms >= 100.0);

    assert_eq!(learning.fleet_size, 12);
    assert!(learning.learned_reliability > learning.baseline_reliability);
    assert!(learning.convergence_rounds <= 3);
    assert!(learning.fleet_wide_adoptions >= 6);

    assert!(alignment.samples >= 4);
    assert!(alignment.mean_rssi_error_db <= 1.1);
    assert!(alignment.mean_snr_error_db <= 0.8);
    assert!(alignment.mean_pdr_error_pct <= 2.5);
    assert!(alignment.mean_latency_error_pct <= 4.0);
    assert!(alignment.alignment_score >= 0.96);

    assert!(budget.compute_path_ns <= 400.0);
    assert!(budget.secure_path_ns <= 400.0);
    assert!(budget.compute_share_of_spi_pct < 3.0);
    assert!(budget.compute_share_of_airtime_pct < 0.001);

    assert_eq!(autonomy.blackout_hours, 72);
    assert!(autonomy.cached_profile_hits >= 25);
    assert_eq!(autonomy.critical_packets_delivered, 30);
    assert!(autonomy.recovery_ticks <= 2);
    assert!(autonomy.restored_delivery_ratio >= 0.99);
    assert!(autonomy.session_recovered_without_controller);
}

#[tokio::test(start_paused = true)]
async fn virtual_10_hop_latency_emulation_stays_within_budget() {
    let kernel = MorphicKernel::new();
    let hop_contexts = [
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
    ];
    let airtime_per_hop = tokio::time::Duration::from_millis(40);
    let start = tokio::time::Instant::now();
    let mut current = radnet_morphic_kernel::PhyProfile::lora_default();

    for context in hop_contexts {
        let target = kernel.planner().generate_phy(&context);
        let mut radio = MockRadio::default();
        kernel.apply_profile(&mut radio, &current, &target).unwrap();
        assert!(radio.standby_called);
        assert!(radio.committed);
        current = target;
        tokio::time::sleep(airtime_per_hop).await;
    }

    let total = start.elapsed();
    assert!(
        total.as_millis() <= 450,
        "Latency exceeds 10-hop target: {total:?}"
    );
}

proptest! {
    #[test]
    fn resilient_interference_points_never_underperform_baseline(index in 0usize..4) {
        let point = &harsh_interference_v2_metrics()[index];
        prop_assert!(point.resilient_successes >= point.baseline_successes);
        prop_assert!(point.resilient_reliability >= 0.90);
    }

    #[test]
    fn tag_budget_monotonically_improves_with_larger_tags(index in 0usize..2) {
        let points = tag_budget_points();
        prop_assert!(points[index].online_forgery_years_50pct < points[index + 1].online_forgery_years_50pct);
    }
}
