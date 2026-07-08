#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub noise_floor_dbm: i16,
    pub snr_db: i8,
    pub battery_mv: u16,
    pub latency_budget_ms: u32,
    pub link_margin_db: i8,
}

impl Context {
    pub fn constrained() -> Self {
        Self {
            noise_floor_dbm: -118,
            snr_db: -8,
            battery_mv: 3300,
            latency_budget_ms: 250,
            link_margin_db: 3,
        }
    }

    pub fn favorable() -> Self {
        Self {
            noise_floor_dbm: -128,
            snr_db: 11,
            battery_mv: 3700,
            latency_budget_ms: 40,
            link_margin_db: 18,
        }
    }
}
