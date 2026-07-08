use crate::context::Context;
use crate::phy::ProfilePreset;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdaptivePowerManager;

impl AdaptivePowerManager {
    pub fn recommend_tx_power_dbm(preset: ProfilePreset, context: &Context) -> i8 {
        let (floor, ceiling) = match preset {
            ProfilePreset::Default => (10, 14),
            ProfilePreset::DefaultLowPower => (8, 12),
            ProfilePreset::LowLatency => (8, 10),
            ProfilePreset::Resilient => (12, 17),
            ProfilePreset::ResilientLowPower => (10, 12),
        };

        let snr_adjusted = if context.snr_db >= 14 && context.link_margin_db >= 20 {
            floor
        } else if context.snr_db >= 8 && context.link_margin_db >= 12 {
            floor + 1
        } else if context.snr_db >= 0 && context.link_margin_db >= 8 {
            (floor + ceiling) / 2
        } else if context.snr_db >= -4 && context.link_margin_db >= 4 {
            ceiling - 1
        } else {
            ceiling
        };

        if context.battery_mv < 3200 {
            snr_adjusted.min(12)
        } else {
            snr_adjusted
        }
    }
}
