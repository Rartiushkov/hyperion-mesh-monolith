use crate::context::Context;
use crate::error::KernelError;
use crate::phy::{CodingRate, Modulation, PhyProfile, ProfileDiff, ProfilePreset, SpreadingFactor};
use crate::power::AdaptivePowerManager;
use crate::radio::{RadioDriver, RegisterWrite, WritePlan};

#[derive(Debug, Default)]
pub struct ProfilePlanner;

impl ProfilePlanner {
    pub fn select_preset(&self, context: &Context) -> ProfilePreset {
        let base = if context.snr_db <= -5 || context.link_margin_db <= 4 {
            ProfilePreset::Resilient
        } else if context.latency_budget_ms <= 60 && context.snr_db >= 8 {
            ProfilePreset::LowLatency
        } else {
            ProfilePreset::Default
        };

        if context.battery_mv < 3200 {
            match base {
                ProfilePreset::Default => ProfilePreset::DefaultLowPower,
                ProfilePreset::Resilient => ProfilePreset::ResilientLowPower,
                other => other,
            }
        } else {
            base
        }
    }

    pub fn generate_phy(&self, context: &Context) -> PhyProfile {
        let preset = self.select_preset(context);
        let mut profile = preset.profile();
        profile.tx_power_dbm = AdaptivePowerManager::recommend_tx_power_dbm(preset, context);
        profile
    }
}

#[derive(Debug, Default)]
pub struct MorphicKernel {
    planner: ProfilePlanner,
}

impl MorphicKernel {
    pub fn new() -> Self {
        Self {
            planner: ProfilePlanner,
        }
    }

    pub fn planner(&self) -> &ProfilePlanner {
        &self.planner
    }

    pub fn apply_profile<D: RadioDriver>(
        &self,
        driver: &mut D,
        current: &PhyProfile,
        target: &PhyProfile,
    ) -> Result<WritePlan, KernelError> {
        current.validate()?;
        target.validate()?;

        self.apply_profile_unchecked(driver, current, target)
    }

    pub fn apply_profile_fast<D: RadioDriver>(
        &self,
        driver: &mut D,
        current: &PhyProfile,
        target: &PhyProfile,
    ) -> Result<WritePlan, KernelError> {
        if let (Some(current_preset), Some(target_preset)) = (current.preset(), target.preset()) {
            return self.apply_preset(driver, current_preset, target_preset);
        }

        self.apply_profile_unchecked(driver, current, target)
    }

    pub fn apply_preset<D: RadioDriver>(
        &self,
        driver: &mut D,
        current: ProfilePreset,
        target: ProfilePreset,
    ) -> Result<WritePlan, KernelError> {
        let writes = self.compile_preset_plan(current, target);
        driver.enter_standby()?;
        driver.write_burst(&writes)?;
        driver.commit()?;
        Ok(writes)
    }

    pub fn compile_preset_plan(&self, current: ProfilePreset, target: ProfilePreset) -> WritePlan {
        compile_preset_transition(current, target)
    }

    fn apply_profile_unchecked<D: RadioDriver>(
        &self,
        driver: &mut D,
        current: &PhyProfile,
        target: &PhyProfile,
    ) -> Result<WritePlan, KernelError> {
        let writes = self.compile_plan(current, target);
        driver.enter_standby()?;
        driver.write_burst(&writes)?;
        driver.commit()?;
        Ok(writes)
    }

    pub fn compile_plan(&self, current: &PhyProfile, target: &PhyProfile) -> WritePlan {
        let diff = current.diff(target);
        if diff.is_empty() {
            return WritePlan::new();
        }

        if let (Some(current_preset), Some(target_preset)) = (current.preset(), target.preset()) {
            return self.compile_preset_plan(current_preset, target_preset);
        }

        self.compile_plan_slow(target, diff)
    }

    fn compile_plan_slow(&self, target: &PhyProfile, diff: ProfileDiff) -> WritePlan {
        let mut writes = WritePlan::new();

        if diff.modulation_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_OP_MODE",
                value: encode_modulation(target.modulation),
            });
        }

        if diff.frequency_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_FRF",
                value: target.frequency_hz,
            });
        }

        if diff.modem_config_1_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_MODEM_CONFIG_1",
                value: encode_config_1(target),
            });
        }

        if diff.modem_config_2_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_MODEM_CONFIG_2",
                value: encode_config_2(target),
            });
        }

        if diff.whitening_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_PACKET_CONFIG",
                value: u32::from(target.whitening_enabled),
            });
        }

        if diff.preamble_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_PREAMBLE",
                value: target.preamble_len as u32,
            });
        }

        if diff.sync_word_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_SYNC_WORD",
                value: target.sync_word as u32,
            });
        }

        if diff.tx_power_changed() {
            let _ = writes.push(RegisterWrite {
                register: "REG_PA_CONFIG",
                value: target.tx_power_dbm as u32,
            });
        }

        writes
    }
}

fn encode_modulation(modulation: Modulation) -> u32 {
    const MODULATION_LUT: [u32; 2] = [0x00, 0x80];
    MODULATION_LUT[modulation as usize]
}

fn encode_config_1(profile: &PhyProfile) -> u32 {
    const BANDWIDTH_LUT: [u32; 4] = [0x06, 0x07, 0x08, 0x09];
    const CODING_RATE_LUT: [u32; 4] = [0x01, 0x02, 0x03, 0x04];
    const HEADER_LUT: [u32; 2] = [0x00, 0x01];
    let bandwidth = BANDWIDTH_LUT[profile.bandwidth as usize];
    let coding_rate = CODING_RATE_LUT[profile.coding_rate.unwrap_or(CodingRate::Cr45) as usize];
    let header = HEADER_LUT[profile.header_mode as usize];

    (bandwidth << 4) | (coding_rate << 1) | header
}

fn encode_config_2(profile: &PhyProfile) -> u32 {
    const SF_LUT: [u32; 6] = [0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C];
    let sf = SF_LUT[profile.spreading_factor.unwrap_or(SpreadingFactor::Sf7) as usize];

    (sf << 4) | ((profile.crc_enabled as u32) << 2)
}

fn compile_preset_transition(current: ProfilePreset, target: ProfilePreset) -> WritePlan {
    let mut writes = WritePlan::new();
    if current == target {
        return writes;
    }

    if preset_config_1(current) != preset_config_1(target) {
        let _ = writes.push(RegisterWrite {
            register: "REG_MODEM_CONFIG_1",
            value: preset_config_1(target),
        });
    }

    if preset_config_2(current) != preset_config_2(target) {
        let _ = writes.push(RegisterWrite {
            register: "REG_MODEM_CONFIG_2",
            value: preset_config_2(target),
        });
    }

    if preset_tx_power(current) != preset_tx_power(target) {
        let _ = writes.push(RegisterWrite {
            register: "REG_PA_CONFIG",
            value: preset_tx_power(target),
        });
    }

    writes
}

const fn preset_config_1(preset: ProfilePreset) -> u32 {
    match preset {
        ProfilePreset::Default | ProfilePreset::DefaultLowPower => 0x72,
        ProfilePreset::LowLatency => 0x82,
        ProfilePreset::Resilient | ProfilePreset::ResilientLowPower => 0x78,
    }
}

const fn preset_config_2(preset: ProfilePreset) -> u32 {
    match preset {
        ProfilePreset::Default | ProfilePreset::DefaultLowPower => 0x94,
        ProfilePreset::LowLatency => 0x74,
        ProfilePreset::Resilient | ProfilePreset::ResilientLowPower => 0xB4,
    }
}

const fn preset_tx_power(preset: ProfilePreset) -> u32 {
    match preset {
        ProfilePreset::Default => 14,
        ProfilePreset::DefaultLowPower => 12,
        ProfilePreset::LowLatency => 10,
        ProfilePreset::Resilient => 17,
        ProfilePreset::ResilientLowPower => 12,
    }
}
