#[cfg(feature = "embedded-hal-adapter")]
pub mod embedded_hal_adapter;
pub mod sx1262_bus;

#[cfg(feature = "embedded-hal-adapter")]
pub use embedded_hal_adapter::{
    EmbeddedHalDelay, EmbeddedHalInputPin, EmbeddedHalOutputPin, EmbeddedHalSpiBus,
};
pub use sx1262_bus::{
    DelayUs, DigitalInput, DigitalOutput, HalSx1262Bus, SpiBus, SX1262_SET_SLEEP_OPCODE,
    SX1262_SET_STANDBY_OPCODE,
};
