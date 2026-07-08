use crate::drivers::RegisterBus;
use crate::error::KernelError;

pub const SX1262_SET_SLEEP_OPCODE: u8 = 0x84;
pub const SX1262_SET_STANDBY_OPCODE: u8 = 0x80;
const SX1262_STANDBY_RC: u8 = 0x00;
const SX1262_BUSY_POLL_LIMIT: usize = 1_024;

pub trait SpiBus {
    fn write(&mut self, words: &[u8]) -> Result<(), KernelError>;
}

pub trait DigitalOutput {
    fn set_low(&mut self) -> Result<(), KernelError>;
    fn set_high(&mut self) -> Result<(), KernelError>;
}

pub trait DigitalInput {
    fn is_high(&self) -> Result<bool, KernelError>;
}

pub trait DelayUs {
    fn delay_us(&mut self, micros: u32);
}

#[derive(Debug)]
pub struct HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY> {
    spi: SPI,
    nss: NSS,
    reset: RESET,
    busy: BUSY,
    delay: DELAY,
}

impl<SPI, NSS, RESET, BUSY, DELAY> HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY> {
    pub fn new(spi: SPI, nss: NSS, reset: RESET, busy: BUSY, delay: DELAY) -> Self {
        Self {
            spi,
            nss,
            reset,
            busy,
            delay,
        }
    }

    pub fn parts(&self) -> (&SPI, &NSS, &RESET, &BUSY, &DELAY) {
        (&self.spi, &self.nss, &self.reset, &self.busy, &self.delay)
    }

    pub fn parts_mut(&mut self) -> (&mut SPI, &mut NSS, &mut RESET, &mut BUSY, &mut DELAY) {
        (
            &mut self.spi,
            &mut self.nss,
            &mut self.reset,
            &mut self.busy,
            &mut self.delay,
        )
    }
}

impl<SPI, NSS, RESET, BUSY, DELAY> HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY>
where
    SPI: SpiBus,
    NSS: DigitalOutput,
    RESET: DigitalOutput,
    BUSY: DigitalInput,
    DELAY: DelayUs,
{
    pub fn hard_reset(&mut self) -> Result<(), KernelError> {
        self.reset.set_low()?;
        self.delay.delay_us(200);
        self.reset.set_high()?;
        self.delay.delay_us(6_000);
        self.wait_while_busy()
    }

    fn wait_while_busy(&mut self) -> Result<(), KernelError> {
        for _ in 0..SX1262_BUSY_POLL_LIMIT {
            if !self.busy.is_high()? {
                return Ok(());
            }
            self.delay.delay_us(50);
        }

        Err(KernelError::Driver("sx1262 busy pin timeout"))
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), KernelError> {
        self.wait_while_busy()?;
        self.nss.set_low()?;
        let write_result = self.spi.write(frame);
        let release_result = self.nss.set_high();

        write_result?;
        release_result?;
        self.wait_while_busy()
    }
}

impl<SPI, NSS, RESET, BUSY, DELAY> RegisterBus for HalSx1262Bus<SPI, NSS, RESET, BUSY, DELAY>
where
    SPI: SpiBus,
    NSS: DigitalOutput,
    RESET: DigitalOutput,
    BUSY: DigitalInput,
    DELAY: DelayUs,
{
    fn enter_standby(&mut self) -> Result<(), KernelError> {
        self.write_frame(&[SX1262_SET_STANDBY_OPCODE, SX1262_STANDBY_RC])
    }

    fn write_opcode(&mut self, opcode: u8, value: u32) -> Result<(), KernelError> {
        let bytes = value.to_be_bytes();
        let frame = [opcode, bytes[0], bytes[1], bytes[2], bytes[3]];
        self.write_frame(&frame)
    }

    fn write_opcode_burst(&mut self, writes: &[(u8, u32)]) -> Result<(), KernelError> {
        for (opcode, value) in writes {
            let bytes = value.to_be_bytes();
            let frame = [*opcode, bytes[0], bytes[1], bytes[2], bytes[3]];
            self.write_frame(&frame)?;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), KernelError> {
        self.wait_while_busy()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DelayUs, DigitalInput, DigitalOutput, HalSx1262Bus, SpiBus, SX1262_SET_STANDBY_OPCODE,
    };
    use crate::drivers::RegisterBus;
    use crate::KernelError;
    use heapless::Vec;

    #[derive(Debug, Default)]
    struct MockSpi {
        frames: Vec<[u8; 5], 16>,
        write_lengths: Vec<usize, 16>,
    }

    impl SpiBus for MockSpi {
        fn write(&mut self, words: &[u8]) -> Result<(), KernelError> {
            let mut frame = [0u8; 5];
            for (idx, byte) in words.iter().enumerate() {
                frame[idx] = *byte;
            }
            let _ = self.frames.push(frame);
            let _ = self.write_lengths.push(words.len());
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
    struct MockBusy {
        high_reads_remaining: usize,
        read_count: usize,
    }

    impl DigitalInput for MockBusy {
        fn is_high(&self) -> Result<bool, KernelError> {
            Ok(self.read_count < self.high_reads_remaining)
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
    fn hal_bus_drives_standby_and_register_frames() {
        let spi = MockSpi::default();
        let nss = MockOutput::default();
        let reset = MockOutput::default();
        let busy = MockBusy::default();
        let delay = MockDelay::default();
        let mut bus = HalSx1262Bus::new(spi, nss, reset, busy, delay);

        bus.enter_standby().unwrap();
        bus.write_opcode(0x86, 868_100_000).unwrap();
        bus.commit().unwrap();

        let (spi, nss, _reset, _busy, _delay) = bus.parts();
        assert_eq!(spi.write_lengths.len(), 2);
        assert_eq!(spi.frames[0][0], SX1262_SET_STANDBY_OPCODE);
        assert_eq!(spi.frames[1][0], 0x86);
        assert_eq!(nss.low_calls, 2);
        assert_eq!(nss.high_calls, 2);
    }

    #[test]
    fn hal_bus_hard_reset_toggles_reset_and_waits() {
        let spi = MockSpi::default();
        let nss = MockOutput::default();
        let reset = MockOutput::default();
        let busy = MockBusy::default();
        let delay = MockDelay::default();
        let mut bus = HalSx1262Bus::new(spi, nss, reset, busy, delay);

        bus.hard_reset().unwrap();

        let (_spi, _nss, reset, _busy, delay) = bus.parts();
        assert_eq!(reset.low_calls, 1);
        assert_eq!(reset.high_calls, 1);
        assert!(delay.delays_us.iter().any(|value| *value == 200));
        assert!(delay.delays_us.iter().any(|value| *value == 6_000));
    }
}
