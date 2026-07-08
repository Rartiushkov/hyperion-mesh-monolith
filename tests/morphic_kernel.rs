use pretty_assertions::assert_eq;
use radnet_morphic_kernel::{
    AdaptivePowerManager, Bandwidth, CodingRate, Context, MockRadio, MorphicKernel, PhyProfile,
    ProfilePreset, SpreadingFactor, ValidationError,
};

#[test]
fn validates_lora_profile_bounds() {
    let mut profile = PhyProfile::lora_default();
    profile.frequency_hz = 42;

    let err = profile.validate().unwrap_err();
    assert_eq!(err, ValidationError::FrequencyOutOfRange(42));
}

#[test]
fn planner_hardens_link_for_bad_conditions() {
    let kernel = MorphicKernel::new();
    let profile = kernel.planner().generate_phy(&Context::constrained());

    assert_eq!(profile.bandwidth, Bandwidth::Khz125);
    assert_eq!(profile.spreading_factor, Some(SpreadingFactor::Sf11));
    assert_eq!(profile.coding_rate, Some(CodingRate::Cr48));
    assert_eq!(profile.tx_power_dbm, 17);
}

#[test]
fn planner_prioritizes_low_latency_for_good_conditions() {
    let kernel = MorphicKernel::new();
    let profile = kernel.planner().generate_phy(&Context::favorable());

    assert_eq!(profile.bandwidth, Bandwidth::Khz250);
    assert_eq!(profile.spreading_factor, Some(SpreadingFactor::Sf7));
    assert_eq!(profile.coding_rate, Some(CodingRate::Cr45));
    assert_eq!(profile.tx_power_dbm, 9);
}

#[test]
fn planner_scales_tx_power_down_for_clean_default_links() {
    let kernel = MorphicKernel::new();
    let profile = kernel.planner().generate_phy(&Context {
        noise_floor_dbm: -129,
        snr_db: 15,
        battery_mv: 3720,
        latency_budget_ms: 120,
        link_margin_db: 22,
    });

    assert_eq!(profile.bandwidth, Bandwidth::Khz125);
    assert_eq!(profile.spreading_factor, Some(SpreadingFactor::Sf9));
    assert_eq!(profile.coding_rate, Some(CodingRate::Cr45));
    assert_eq!(profile.tx_power_dbm, 10);
}

#[test]
fn adaptive_power_manager_clamps_low_battery_profiles() {
    let power = AdaptivePowerManager::recommend_tx_power_dbm(
        ProfilePreset::Default,
        &Context {
            noise_floor_dbm: -112,
            snr_db: -6,
            battery_mv: 3050,
            latency_budget_ms: 140,
            link_margin_db: 2,
        },
    );

    assert_eq!(power, 12);
}

#[test]
fn compile_plan_rebalances_only_pa_config_when_same_profile_family_changes_power() {
    let kernel = MorphicKernel::new();
    let clean_context = Context {
        noise_floor_dbm: -129,
        snr_db: 15,
        battery_mv: 3720,
        latency_budget_ms: 120,
        link_margin_db: 22,
    };
    let noisier_context = Context {
        noise_floor_dbm: -118,
        snr_db: 1,
        battery_mv: 3720,
        latency_budget_ms: 120,
        link_margin_db: 8,
    };

    let current = kernel.planner().generate_phy(&clean_context);
    let target = kernel.planner().generate_phy(&noisier_context);
    let writes = kernel.compile_plan(&current, &target);

    assert_eq!(current.bandwidth, target.bandwidth);
    assert_eq!(current.spreading_factor, target.spreading_factor);
    assert_eq!(current.coding_rate, target.coding_rate);
    assert!(target.tx_power_dbm > current.tx_power_dbm);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].register, "REG_PA_CONFIG");
    assert_eq!(writes[0].value, target.tx_power_dbm as u32);
}

#[test]
fn compile_plan_only_emits_changed_registers() {
    let kernel = MorphicKernel::new();
    let current = PhyProfile::lora_default();
    let mut target = current.clone();
    target.bandwidth = Bandwidth::Khz250;
    target.spreading_factor = Some(SpreadingFactor::Sf7);
    target.tx_power_dbm = 12;

    let writes = kernel.compile_plan(&current, &target);
    let registers: Vec<_> = writes.into_iter().map(|w| w.register).collect();

    assert_eq!(
        registers,
        vec!["REG_MODEM_CONFIG_1", "REG_MODEM_CONFIG_2", "REG_PA_CONFIG"]
    );
}

#[test]
fn apply_profile_enters_standby_writes_registers_and_commits() {
    let kernel = MorphicKernel::new();
    let current = PhyProfile::lora_default();
    let mut target = current.clone();
    target.frequency_hz = 915_000_000;
    target.preamble_len = 12;

    let mut radio = MockRadio::default();
    let writes = kernel.apply_profile(&mut radio, &current, &target).unwrap();

    assert!(radio.standby_called);
    assert!(radio.committed);
    assert_eq!(radio.writes, writes);
    assert_eq!(radio.writes.len(), 2);
    assert_eq!(radio.writes[0].register, "REG_FRF");
    assert_eq!(radio.writes[1].register, "REG_PREAMBLE");
}
