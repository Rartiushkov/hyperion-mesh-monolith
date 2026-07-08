use crate::error::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Modulation {
    Fsk,
    LoRa,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Bandwidth {
    Khz62,
    Khz125,
    Khz250,
    Khz500,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpreadingFactor {
    Sf7,
    Sf8,
    Sf9,
    Sf10,
    Sf11,
    Sf12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodingRate {
    Cr45,
    Cr46,
    Cr47,
    Cr48,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HeaderMode {
    Explicit,
    Implicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhyProfile {
    pub modulation: Modulation,
    pub frequency_hz: u32,
    pub bandwidth: Bandwidth,
    pub spreading_factor: Option<SpreadingFactor>,
    pub coding_rate: Option<CodingRate>,
    pub preamble_len: u16,
    pub sync_word: u8,
    pub tx_power_dbm: i8,
    pub crc_enabled: bool,
    pub whitening_enabled: bool,
    pub header_mode: HeaderMode,
}

impl PhyProfile {
    pub const fn lora_default() -> Self {
        ProfilePreset::Default.profile()
    }

    pub fn preset(self) -> Option<ProfilePreset> {
        let base_profile = self.matches_common_lora_envelope();
        if !base_profile {
            return None;
        }

        match (
            self.bandwidth,
            self.spreading_factor,
            self.coding_rate,
            self.tx_power_dbm,
        ) {
            (Bandwidth::Khz125, Some(SpreadingFactor::Sf9), Some(CodingRate::Cr45), 14) => {
                Some(ProfilePreset::Default)
            }
            (Bandwidth::Khz125, Some(SpreadingFactor::Sf9), Some(CodingRate::Cr45), 12) => {
                Some(ProfilePreset::DefaultLowPower)
            }
            (Bandwidth::Khz250, Some(SpreadingFactor::Sf7), Some(CodingRate::Cr45), 10) => {
                Some(ProfilePreset::LowLatency)
            }
            (Bandwidth::Khz125, Some(SpreadingFactor::Sf11), Some(CodingRate::Cr48), 17) => {
                Some(ProfilePreset::Resilient)
            }
            (Bandwidth::Khz125, Some(SpreadingFactor::Sf11), Some(CodingRate::Cr48), 12) => {
                Some(ProfilePreset::ResilientLowPower)
            }
            _ => None,
        }
    }

    fn matches_common_lora_envelope(self) -> bool {
        matches!(self.modulation, Modulation::LoRa)
            && self.frequency_hz == 868_100_000
            && self.preamble_len == 8
            && self.sync_word == 0x12
            && self.crc_enabled
            && !self.whitening_enabled
            && matches!(self.header_mode, HeaderMode::Explicit)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(137_000_000..=1_020_000_000).contains(&self.frequency_hz) {
            return Err(ValidationError::FrequencyOutOfRange(self.frequency_hz));
        }

        if !(-4..=20).contains(&self.tx_power_dbm) {
            return Err(ValidationError::PowerOutOfRange(self.tx_power_dbm));
        }

        if self.preamble_len == 0 {
            return Err(ValidationError::EmptyPreamble);
        }

        match self.modulation {
            Modulation::LoRa => {
                let Some(sf) = self.spreading_factor else {
                    return Err(ValidationError::IncompatibleModulation(self.modulation));
                };
                let Some(_cr) = self.coding_rate else {
                    return Err(ValidationError::IncompatibleModulation(self.modulation));
                };

                if !matches!(
                    sf,
                    SpreadingFactor::Sf7
                        | SpreadingFactor::Sf8
                        | SpreadingFactor::Sf9
                        | SpreadingFactor::Sf10
                        | SpreadingFactor::Sf11
                        | SpreadingFactor::Sf12
                ) {
                    return Err(ValidationError::UnsupportedSpreadingFactor(sf));
                }

                if !matches!(
                    self.bandwidth,
                    Bandwidth::Khz62 | Bandwidth::Khz125 | Bandwidth::Khz250 | Bandwidth::Khz500
                ) {
                    return Err(ValidationError::UnsupportedBandwidth(self.bandwidth));
                }
            }
            Modulation::Fsk => {
                if self.spreading_factor.is_some() || self.coding_rate.is_some() {
                    return Err(ValidationError::IncompatibleModulation(self.modulation));
                }
            }
        }

        Ok(())
    }

    pub fn diff(&self, target: &Self) -> ProfileDiff {
        ProfileDiff::between(self, target)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfilePreset {
    Default,
    DefaultLowPower,
    LowLatency,
    Resilient,
    ResilientLowPower,
}

impl ProfilePreset {
    pub const fn profile(self) -> PhyProfile {
        match self {
            Self::Default => PhyProfile {
                modulation: Modulation::LoRa,
                frequency_hz: 868_100_000,
                bandwidth: Bandwidth::Khz125,
                spreading_factor: Some(SpreadingFactor::Sf9),
                coding_rate: Some(CodingRate::Cr45),
                preamble_len: 8,
                sync_word: 0x12,
                tx_power_dbm: 14,
                crc_enabled: true,
                whitening_enabled: false,
                header_mode: HeaderMode::Explicit,
            },
            Self::DefaultLowPower => PhyProfile {
                modulation: Modulation::LoRa,
                frequency_hz: 868_100_000,
                bandwidth: Bandwidth::Khz125,
                spreading_factor: Some(SpreadingFactor::Sf9),
                coding_rate: Some(CodingRate::Cr45),
                preamble_len: 8,
                sync_word: 0x12,
                tx_power_dbm: 12,
                crc_enabled: true,
                whitening_enabled: false,
                header_mode: HeaderMode::Explicit,
            },
            Self::LowLatency => PhyProfile {
                modulation: Modulation::LoRa,
                frequency_hz: 868_100_000,
                bandwidth: Bandwidth::Khz250,
                spreading_factor: Some(SpreadingFactor::Sf7),
                coding_rate: Some(CodingRate::Cr45),
                preamble_len: 8,
                sync_word: 0x12,
                tx_power_dbm: 10,
                crc_enabled: true,
                whitening_enabled: false,
                header_mode: HeaderMode::Explicit,
            },
            Self::Resilient => PhyProfile {
                modulation: Modulation::LoRa,
                frequency_hz: 868_100_000,
                bandwidth: Bandwidth::Khz125,
                spreading_factor: Some(SpreadingFactor::Sf11),
                coding_rate: Some(CodingRate::Cr48),
                preamble_len: 8,
                sync_word: 0x12,
                tx_power_dbm: 17,
                crc_enabled: true,
                whitening_enabled: false,
                header_mode: HeaderMode::Explicit,
            },
            Self::ResilientLowPower => PhyProfile {
                modulation: Modulation::LoRa,
                frequency_hz: 868_100_000,
                bandwidth: Bandwidth::Khz125,
                spreading_factor: Some(SpreadingFactor::Sf11),
                coding_rate: Some(CodingRate::Cr48),
                preamble_len: 8,
                sync_word: 0x12,
                tx_power_dbm: 12,
                crc_enabled: true,
                whitening_enabled: false,
                header_mode: HeaderMode::Explicit,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileDiff(u16);

impl ProfileDiff {
    const MODULATION: u16 = 1 << 0;
    const FREQUENCY: u16 = 1 << 1;
    const BANDWIDTH: u16 = 1 << 2;
    const SPREADING_FACTOR: u16 = 1 << 3;
    const CODING_RATE: u16 = 1 << 4;
    const PREAMBLE: u16 = 1 << 5;
    const SYNC_WORD: u16 = 1 << 6;
    const TX_POWER: u16 = 1 << 7;
    const CRC: u16 = 1 << 8;
    const WHITENING: u16 = 1 << 9;
    const HEADER: u16 = 1 << 10;

    pub fn between(current: &PhyProfile, target: &PhyProfile) -> Self {
        let mut bits = 0u16;
        bits |= ((current.modulation != target.modulation) as u16) * Self::MODULATION;
        bits |= ((current.frequency_hz != target.frequency_hz) as u16) * Self::FREQUENCY;
        bits |= ((current.bandwidth != target.bandwidth) as u16) * Self::BANDWIDTH;
        bits |=
            ((current.spreading_factor != target.spreading_factor) as u16) * Self::SPREADING_FACTOR;
        bits |= ((current.coding_rate != target.coding_rate) as u16) * Self::CODING_RATE;
        bits |= ((current.preamble_len != target.preamble_len) as u16) * Self::PREAMBLE;
        bits |= ((current.sync_word != target.sync_word) as u16) * Self::SYNC_WORD;
        bits |= ((current.tx_power_dbm != target.tx_power_dbm) as u16) * Self::TX_POWER;
        bits |= ((current.crc_enabled != target.crc_enabled) as u16) * Self::CRC;
        bits |= ((current.whitening_enabled != target.whitening_enabled) as u16) * Self::WHITENING;
        bits |= ((current.header_mode != target.header_mode) as u16) * Self::HEADER;
        Self(bits)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn modulation_changed(self) -> bool {
        self.has(Self::MODULATION)
    }

    pub const fn frequency_changed(self) -> bool {
        self.has(Self::FREQUENCY)
    }

    pub const fn modem_config_1_changed(self) -> bool {
        self.has(Self::BANDWIDTH | Self::CODING_RATE | Self::HEADER)
    }

    pub const fn modem_config_2_changed(self) -> bool {
        self.has(Self::SPREADING_FACTOR | Self::CRC)
    }

    pub const fn whitening_changed(self) -> bool {
        self.has(Self::WHITENING)
    }

    pub const fn preamble_changed(self) -> bool {
        self.has(Self::PREAMBLE)
    }

    pub const fn sync_word_changed(self) -> bool {
        self.has(Self::SYNC_WORD)
    }

    pub const fn tx_power_changed(self) -> bool {
        self.has(Self::TX_POWER)
    }

    const fn has(self, mask: u16) -> bool {
        self.0 & mask != 0
    }
}
