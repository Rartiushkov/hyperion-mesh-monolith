use crate::phy::{Bandwidth, Modulation, SpreadingFactor};
use core::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    FrequencyOutOfRange(u32),
    PowerOutOfRange(i8),
    UnsupportedSpreadingFactor(SpreadingFactor),
    UnsupportedBandwidth(Bandwidth),
    IncompatibleModulation(Modulation),
    EmptyPreamble,
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FrequencyOutOfRange(freq) => write!(f, "frequency out of range: {freq} Hz"),
            Self::PowerOutOfRange(power) => write!(f, "tx power out of range: {power} dBm"),
            Self::UnsupportedSpreadingFactor(sf) => {
                write!(f, "unsupported spreading factor: {:?}", sf)
            }
            Self::UnsupportedBandwidth(bw) => write!(f, "unsupported bandwidth: {:?}", bw),
            Self::IncompatibleModulation(mode) => {
                write!(f, "profile contains unsupported fields for {:?}", mode)
            }
            Self::EmptyPreamble => write!(f, "preamble must be at least 1 symbol"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ValidationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelError {
    Validation(ValidationError),
    Driver(&'static str),
}

impl Display for KernelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Validation(err) => write!(f, "{err}"),
            Self::Driver(msg) => write!(f, "{msg}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for KernelError {}

impl From<ValidationError> for KernelError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}
