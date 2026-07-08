use crate::drivers::{LogSink, Sx1262Driver};
use crate::error::KernelError;
use crate::hal::{DelayUs, DigitalInput, DigitalOutput, HalSx1262Bus, SpiBus};
use crate::radio::RadioDriver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Esp32C3BoardConfig {
    pub node_id: &'static str,
    pub hardware_id: &'static str,
    pub radio_frequency_hz: u32,
    pub spi_clock_hz: u32,
}

impl Default for Esp32C3BoardConfig {
    fn default() -> Self {
        Self {
            node_id: "RDN-ESP32C3-001",
            hardware_id: "esp32c3_sx1262_devkit",
            radio_frequency_hz: 868_100_000,
            spi_clock_hz: 8_000_000,
        }
    }
}

#[derive(Debug)]
pub struct Esp32C3RadioPins<NSS, RESET, BUSY> {
    pub nss: NSS,
    pub reset: RESET,
    pub busy: BUSY,
}

impl<NSS, RESET, BUSY> Esp32C3RadioPins<NSS, RESET, BUSY> {
    pub fn new(nss: NSS, reset: RESET, busy: BUSY) -> Self {
        Self { nss, reset, busy }
    }
}

#[derive(Debug)]
pub struct Esp32C3Sx1262Board<SPI, NSS, RESET, BUSY, DELAY, L> {
    config: Esp32C3BoardConfig,
    driver: Sx1262Driver<HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY>, L>,
}

impl<SPI, NSS, RESET, BUSY, DELAY, L> Esp32C3Sx1262Board<SPI, NSS, RESET, BUSY, DELAY, L>
where
    SPI: SpiBus,
    NSS: DigitalOutput,
    RESET: DigitalOutput,
    BUSY: DigitalInput,
    DELAY: DelayUs,
    L: LogSink,
{
    pub fn new(
        config: Esp32C3BoardConfig,
        spi: SPI,
        pins: Esp32C3RadioPins<NSS, RESET, BUSY>,
        delay: DELAY,
        log_sink: L,
    ) -> Self {
        let bus = HalSx1262Bus::new(spi, pins.nss, pins.reset, pins.busy, delay);
        let driver = Sx1262Driver::new(config.node_id, config.hardware_id, bus, log_sink);

        Self { config, driver }
    }

    pub fn config(&self) -> &Esp32C3BoardConfig {
        &self.config
    }

    pub fn driver(&self) -> &Sx1262Driver<HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY>, L> {
        &self.driver
    }

    pub fn driver_mut(
        &mut self,
    ) -> &mut Sx1262Driver<HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY>, L> {
        &mut self.driver
    }

    pub fn bring_up(&mut self) -> Result<(), KernelError> {
        self.driver.bus_mut().hard_reset()?;
        self.driver.enter_standby()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Esp32C3BoardConfig, Esp32C3RadioPins, Esp32C3Sx1262Board};
    use crate::drivers::MemoryLogSink;
    use crate::hal::{DelayUs, DigitalInput, DigitalOutput, SpiBus};
    use crate::KernelError;
    use heapless::Vec;

    #[derive(Debug, Default)]
    struct MockSpi {
        frames: Vec<[u8; 5], 16>,
    }

    impl SpiBus for MockSpi {
        fn write(&mut self, words: &[u8]) -> Result<(), KernelError> {
            let mut frame = [0u8; 5];
            for (index, byte) in words.iter().enumerate() {
                frame[index] = *byte;
            }
            let _ = self.frames.push(frame);
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct MockOutput {
        low_calls: usize,
        high_calls: usize,
    }

    impl DigitalOutput for MockOutput {
        fn set_low(&mut self) -> Result<(), KernelError> {
            self.low_calls += 1;
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), KernelError> {
            self.high_calls += 1;
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct MockBusy;

    impl DigitalInput for MockBusy {
        fn is_high(&self) -> Result<bool, KernelError> {
            Ok(false)
        }
    }

    #[derive(Debug, Default)]
    struct MockDelay {
        delays_us: Vec<u32, 32>,
    }

    impl DelayUs for MockDelay {
        fn delay_us(&mut self, micros: u32) {
            let _ = self.delays_us.push(micros);
        }
    }

    #[test]
    fn esp32c3_board_scaffold_brings_up_radio_bus() {
        let config = Esp32C3BoardConfig::default();
        let spi = MockSpi::default();
        let pins = Esp32C3RadioPins::new(MockOutput::default(), MockOutput::default(), MockBusy);
        let delay = MockDelay::default();
        let sink = MemoryLogSink::default();
        let mut board = Esp32C3Sx1262Board::new(config, spi, pins, delay, sink);

        board.bring_up().unwrap();

        let (spi, nss, reset, _busy, delay) = board.driver().bus().parts();
        assert_eq!(board.config().hardware_id, "esp32c3_sx1262_devkit");
        assert_eq!(spi.frames.len(), 1);
        assert_eq!(nss.low_calls, 1);
        assert_eq!(nss.high_calls, 1);
        assert_eq!(reset.low_calls, 1);
        assert_eq!(reset.high_calls, 1);
        assert!(delay.delays_us.iter().any(|value| *value == 6_000));
    }
}
