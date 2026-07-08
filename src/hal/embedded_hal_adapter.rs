use crate::error::KernelError;
use crate::hal::{DelayUs, DigitalInput, DigitalOutput, SpiBus};
use core::cell::{Ref, RefCell, RefMut};
use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiBus as EmbeddedSpiBus;

#[derive(Debug)]
pub struct EmbeddedHalSpiBus<SPI> {
    inner: SPI,
}

impl<SPI> EmbeddedHalSpiBus<SPI> {
    pub fn new(inner: SPI) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &SPI {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut SPI {
        &mut self.inner
    }

    pub fn release(self) -> SPI {
        self.inner
    }
}

impl<SPI> SpiBus for EmbeddedHalSpiBus<SPI>
where
    SPI: EmbeddedSpiBus<u8>,
{
    fn write(&mut self, words: &[u8]) -> Result<(), KernelError> {
        self.inner
            .write(words)
            .map_err(|_| KernelError::Driver("embedded-hal spi write failed"))
    }
}

#[derive(Debug)]
pub struct EmbeddedHalOutputPin<PIN> {
    inner: PIN,
}

impl<PIN> EmbeddedHalOutputPin<PIN> {
    pub fn new(inner: PIN) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &PIN {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut PIN {
        &mut self.inner
    }

    pub fn release(self) -> PIN {
        self.inner
    }
}

impl<PIN> DigitalOutput for EmbeddedHalOutputPin<PIN>
where
    PIN: OutputPin,
{
    fn set_low(&mut self) -> Result<(), KernelError> {
        self.inner
            .set_low()
            .map_err(|_| KernelError::Driver("embedded-hal output pin set_low failed"))
    }

    fn set_high(&mut self) -> Result<(), KernelError> {
        self.inner
            .set_high()
            .map_err(|_| KernelError::Driver("embedded-hal output pin set_high failed"))
    }
}

#[derive(Debug)]
pub struct EmbeddedHalInputPin<PIN> {
    inner: RefCell<PIN>,
}

impl<PIN> EmbeddedHalInputPin<PIN> {
    pub fn new(inner: PIN) -> Self {
        Self {
            inner: RefCell::new(inner),
        }
    }

    pub fn borrow(&self) -> Ref<'_, PIN> {
        self.inner.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, PIN> {
        self.inner.borrow_mut()
    }

    pub fn release(self) -> PIN {
        self.inner.into_inner()
    }
}

impl<PIN> DigitalInput for EmbeddedHalInputPin<PIN>
where
    PIN: InputPin,
{
    fn is_high(&self) -> Result<bool, KernelError> {
        self.inner
            .borrow_mut()
            .is_high()
            .map_err(|_| KernelError::Driver("embedded-hal input pin read failed"))
    }
}

#[derive(Debug)]
pub struct EmbeddedHalDelay<DELAY> {
    inner: DELAY,
}

impl<DELAY> EmbeddedHalDelay<DELAY> {
    pub fn new(inner: DELAY) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &DELAY {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut DELAY {
        &mut self.inner
    }

    pub fn release(self) -> DELAY {
        self.inner
    }
}

impl<DELAY> DelayUs for EmbeddedHalDelay<DELAY>
where
    DELAY: DelayNs,
{
    fn delay_us(&mut self, micros: u32) {
        self.inner.delay_us(micros);
    }
}

#[cfg(test)]
#[cfg(feature = "embedded-hal-adapter")]
mod tests {
    use super::{EmbeddedHalDelay, EmbeddedHalInputPin, EmbeddedHalOutputPin, EmbeddedHalSpiBus};
    use crate::hal::{DelayUs, DigitalInput, DigitalOutput, SpiBus};
    use core::convert::Infallible;
    use embedded_hal::delay::DelayNs;
    use embedded_hal::digital::{ErrorType as DigitalErrorType, InputPin, OutputPin};
    use embedded_hal::spi::{ErrorType as SpiErrorType, SpiBus as EmbeddedSpiBus};
    use heapless::Vec;

    #[derive(Debug, Default)]
    struct MockEhSpi {
        writes: Vec<[u8; 8], 8>,
    }

    impl SpiErrorType for MockEhSpi {
        type Error = Infallible;
    }

    impl EmbeddedSpiBus<u8> for MockEhSpi {
        fn read(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            for word in words.iter_mut() {
                *word = 0;
            }
            Ok(())
        }

        fn write(&mut self, words: &[u8]) -> Result<(), Self::Error> {
            let mut frame = [0u8; 8];
            for (idx, byte) in words.iter().enumerate() {
                frame[idx] = *byte;
            }
            let _ = self.writes.push(frame);
            Ok(())
        }

        fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Self::Error> {
            let _ = self.write(write);
            for (dst, src) in read.iter_mut().zip(write.iter().copied()) {
                *dst = src;
            }
            Ok(())
        }

        fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            let copy = words.to_vec();
            let _ = self.write(&copy);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct MockEhOutput {
        low_calls: usize,
        high_calls: usize,
    }

    impl DigitalErrorType for MockEhOutput {
        type Error = Infallible;
    }

    impl OutputPin for MockEhOutput {
        fn set_low(&mut self) -> Result<(), Self::Error> {
            self.low_calls += 1;
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), Self::Error> {
            self.high_calls += 1;
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct MockEhInput {
        high: bool,
    }

    impl DigitalErrorType for MockEhInput {
        type Error = Infallible;
    }

    impl InputPin for MockEhInput {
        fn is_high(&mut self) -> Result<bool, Self::Error> {
            Ok(self.high)
        }

        fn is_low(&mut self) -> Result<bool, Self::Error> {
            Ok(!self.high)
        }
    }

    #[derive(Debug, Default)]
    struct MockEhDelay {
        total_us: u32,
    }

    impl DelayNs for MockEhDelay {
        fn delay_ns(&mut self, ns: u32) {
            self.total_us = self.total_us.saturating_add(ns / 1_000);
        }
    }

    #[test]
    fn embedded_hal_wrappers_bridge_to_local_hal_traits() {
        let mut spi = EmbeddedHalSpiBus::new(MockEhSpi::default());
        let mut output = EmbeddedHalOutputPin::new(MockEhOutput::default());
        let input = EmbeddedHalInputPin::new(MockEhInput::default());
        let mut delay = EmbeddedHalDelay::new(MockEhDelay::default());

        spi.write(&[0xAA, 0x55]).unwrap();
        output.set_low().unwrap();
        output.set_high().unwrap();
        assert!(!input.is_high().unwrap());
        delay.delay_us(250);

        assert_eq!(spi.inner().writes.len(), 1);
        assert_eq!(output.inner().low_calls, 1);
        assert_eq!(output.inner().high_calls, 1);
        assert!(delay.inner().total_us >= 250);
    }
}
