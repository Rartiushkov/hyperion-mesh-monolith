use crate::error::KernelError;
use heapless::Vec;

const REG_FIFO: u8 = 0x00;
const REG_OP_MODE: u8 = 0x01;
const REG_FRF_MSB: u8 = 0x06;
const REG_FRF_MID: u8 = 0x07;
const REG_FRF_LSB: u8 = 0x08;
const REG_PA_CONFIG: u8 = 0x09;
const REG_OCP: u8 = 0x0B;
const REG_LNA: u8 = 0x0C;
const REG_FIFO_ADDR_PTR: u8 = 0x0D;
const REG_FIFO_TX_BASE_ADDR: u8 = 0x0E;
const REG_FIFO_RX_BASE_ADDR: u8 = 0x0F;
const REG_FIFO_RX_CURRENT_ADDR: u8 = 0x10;
const REG_IRQ_FLAGS: u8 = 0x12;
const REG_RX_NB_BYTES: u8 = 0x13;
const REG_PKT_SNR_VALUE: u8 = 0x19;
const REG_PKT_RSSI_VALUE: u8 = 0x1A;
const REG_MODEM_CONFIG_1: u8 = 0x1D;
const REG_MODEM_CONFIG_2: u8 = 0x1E;
const REG_PREAMBLE_MSB: u8 = 0x20;
const REG_PREAMBLE_LSB: u8 = 0x21;
const REG_PAYLOAD_LENGTH: u8 = 0x22;
const REG_MAX_PAYLOAD_LENGTH: u8 = 0x23;
const REG_MODEM_CONFIG_3: u8 = 0x26;
const REG_PA_DAC: u8 = 0x4D;

const MODE_LONG_RANGE: u8 = 0x80;
const MODE_SLEEP: u8 = 0x00;
const MODE_STDBY: u8 = 0x01;
const MODE_TX: u8 = 0x03;
const MODE_RX_CONTINUOUS: u8 = 0x05;

const IRQ_RX_DONE: u8 = 0x40;
const IRQ_PAYLOAD_CRC_ERROR: u8 = 0x20;

const OSCILLATOR_HZ: u64 = 32_000_000;
const FRF_SCALE: u64 = 1 << 19;

pub trait Sx1276SpiBus {
    fn write(&mut self, bytes: &[u8]) -> Result<(), KernelError>;
    fn transfer(&mut self, bytes: &mut [u8]) -> Result<(), KernelError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawListenerConfig {
    pub frequency_hz: u32,
    pub implicit_payload_len: u8,
    pub preamble_symbols: u16,
    pub max_payload_len: u8,
}

impl Default for RawListenerConfig {
    fn default() -> Self {
        Self {
            frequency_hz: 868_100_000,
            implicit_payload_len: 32,
            preamble_symbols: 8,
            max_payload_len: 255,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPacketSample {
    pub timestamp_ms: u64,
    pub bytes: Vec<u8, 255>,
    pub rssi_dbm: i16,
    pub snr_quarter_db: i16,
    pub irq_flags: u8,
    pub crc_error_observed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sx1276RawPacketRing<const N: usize> {
    samples: Vec<RawPacketSample, N>,
    next_overwrite: usize,
    dropped_samples: u32,
}

impl<const N: usize> Default for Sx1276RawPacketRing<N> {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            next_overwrite: 0,
            dropped_samples: 0,
        }
    }
}

impl<const N: usize> Sx1276RawPacketRing<N> {
    pub fn push(&mut self, sample: RawPacketSample) {
        if self.samples.push(sample.clone()).is_ok() {
            return;
        }
        if N == 0 {
            self.dropped_samples = self.dropped_samples.saturating_add(1);
            return;
        }
        self.samples[self.next_overwrite] = sample;
        self.next_overwrite = (self.next_overwrite + 1) % N;
        self.dropped_samples = self.dropped_samples.saturating_add(1);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn dropped_samples(&self) -> u32 {
        self.dropped_samples
    }

    pub fn samples(&self) -> &[RawPacketSample] {
        &self.samples
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaThermalCwConfig {
    pub frequency_hz: u32,
    pub pa_output_dbm: i8,
    pub max_duration_ms: u32,
    pub regulatory_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaThermalPlan {
    pub frequency_hz: u32,
    pub pa_output_dbm: i8,
    pub max_duration_ms: u32,
    pub continuous_wave_started: bool,
    pub guard_reason: &'static str,
}

#[derive(Debug)]
pub struct Sx1276Driver<B, const RING: usize> {
    bus: B,
    raw_ring: Sx1276RawPacketRing<RING>,
}

impl<B, const RING: usize> Sx1276Driver<B, RING> {
    pub fn new(bus: B) -> Self {
        Self {
            bus,
            raw_ring: Sx1276RawPacketRing::default(),
        }
    }

    pub fn bus(&self) -> &B {
        &self.bus
    }

    pub fn bus_mut(&mut self) -> &mut B {
        &mut self.bus
    }

    pub fn raw_ring(&self) -> &Sx1276RawPacketRing<RING> {
        &self.raw_ring
    }
}

impl<B: Sx1276SpiBus, const RING: usize> Sx1276Driver<B, RING> {
    pub fn configure_raw_packet_listener(
        &mut self,
        config: &RawListenerConfig,
    ) -> Result<(), KernelError> {
        validate_frequency(config.frequency_hz)?;
        if config.implicit_payload_len == 0 {
            return Err(KernelError::Driver(
                "SX1276 implicit payload length is zero",
            ));
        }

        self.write_register(REG_OP_MODE, MODE_LONG_RANGE | MODE_SLEEP)?;
        self.set_frequency_hz(config.frequency_hz)?;
        self.write_register(REG_FIFO_RX_BASE_ADDR, 0x00)?;
        self.write_register(REG_FIFO_TX_BASE_ADDR, 0x80)?;
        self.write_register(REG_FIFO_ADDR_PTR, 0x00)?;
        self.write_register(REG_PREAMBLE_MSB, (config.preamble_symbols >> 8) as u8)?;
        self.write_register(REG_PREAMBLE_LSB, config.preamble_symbols as u8)?;
        self.write_register(REG_PAYLOAD_LENGTH, config.implicit_payload_len)?;
        self.write_register(REG_MAX_PAYLOAD_LENGTH, config.max_payload_len)?;

        let mut modem_config_1 = self.read_register(REG_MODEM_CONFIG_1)?;
        modem_config_1 |= 0x01;
        self.write_register(REG_MODEM_CONFIG_1, modem_config_1)?;

        let mut modem_config_2 = self.read_register(REG_MODEM_CONFIG_2)?;
        modem_config_2 &= !0x04;
        self.write_register(REG_MODEM_CONFIG_2, modem_config_2)?;
        self.write_register(REG_MODEM_CONFIG_3, 0x04)?;
        let lna = self.read_register(REG_LNA)? | 0x03;
        self.write_register(REG_LNA, lna)?;
        self.write_register(REG_IRQ_FLAGS, 0xFF)?;
        self.write_register(REG_OP_MODE, MODE_LONG_RANGE | MODE_RX_CONTINUOUS)?;
        Ok(())
    }

    pub fn poll_raw_packet(
        &mut self,
        timestamp_ms: u64,
    ) -> Result<Option<RawPacketSample>, KernelError> {
        let irq_flags = self.read_register(REG_IRQ_FLAGS)?;
        if irq_flags & IRQ_RX_DONE == 0 {
            return Ok(None);
        }

        let byte_count = self.read_register(REG_RX_NB_BYTES)?;
        let current_addr = self.read_register(REG_FIFO_RX_CURRENT_ADDR)?;
        self.write_register(REG_FIFO_ADDR_PTR, current_addr)?;

        let mut bytes = Vec::<u8, 255>::new();
        for _ in 0..byte_count {
            bytes
                .push(self.read_register(REG_FIFO)?)
                .map_err(|_| KernelError::Driver("SX1276 raw packet buffer full"))?;
        }

        let snr_quarter_db = self.read_register(REG_PKT_SNR_VALUE)? as i8 as i16;
        let rssi_dbm = -157 + self.read_register(REG_PKT_RSSI_VALUE)? as i16;
        self.write_register(REG_IRQ_FLAGS, irq_flags)?;

        let sample = RawPacketSample {
            timestamp_ms,
            bytes,
            rssi_dbm,
            snr_quarter_db,
            irq_flags,
            crc_error_observed: irq_flags & IRQ_PAYLOAD_CRC_ERROR != 0,
        };
        self.raw_ring.push(sample.clone());
        Ok(Some(sample))
    }

    pub fn start_pa_thermal_cw(
        &mut self,
        config: PaThermalCwConfig,
    ) -> Result<PaThermalPlan, KernelError> {
        validate_frequency(config.frequency_hz)?;
        if !config.regulatory_acknowledged {
            return Err(KernelError::Driver(
                "SX1276 CW lab mode requires explicit regulatory acknowledgement",
            ));
        }
        if config.max_duration_ms == 0 || config.max_duration_ms > 30_000 {
            return Err(KernelError::Driver(
                "SX1276 CW lab mode duration must be 1..30000 ms",
            ));
        }
        if !(-4..=20).contains(&config.pa_output_dbm) {
            return Err(KernelError::Driver(
                "SX1276 PA output outside guarded range",
            ));
        }

        self.write_register(REG_OP_MODE, MODE_LONG_RANGE | MODE_SLEEP)?;
        self.set_frequency_hz(config.frequency_hz)?;
        self.write_register(REG_PA_CONFIG, pa_config_for_dbm(config.pa_output_dbm))?;
        self.write_register(REG_OCP, 0x2B)?;
        self.write_register(
            REG_PA_DAC,
            if config.pa_output_dbm > 17 {
                0x87
            } else {
                0x84
            },
        )?;
        self.write_register(REG_OP_MODE, MODE_LONG_RANGE | MODE_STDBY)?;
        self.write_register(REG_OP_MODE, MODE_LONG_RANGE | MODE_TX)?;

        Ok(PaThermalPlan {
            frequency_hz: config.frequency_hz,
            pa_output_dbm: config.pa_output_dbm,
            max_duration_ms: config.max_duration_ms,
            continuous_wave_started: true,
            guard_reason: "bounded lab CW for PA heat/SWR calibration only",
        })
    }

    pub fn stop_pa_thermal_cw(&mut self) -> Result<(), KernelError> {
        self.write_register(REG_OP_MODE, MODE_LONG_RANGE | MODE_STDBY)
    }

    pub fn set_frequency_hz(&mut self, frequency_hz: u32) -> Result<(), KernelError> {
        validate_frequency(frequency_hz)?;
        let frf = ((frequency_hz as u64) * FRF_SCALE / OSCILLATOR_HZ) as u32;
        self.write_register(REG_FRF_MSB, ((frf >> 16) & 0xFF) as u8)?;
        self.write_register(REG_FRF_MID, ((frf >> 8) & 0xFF) as u8)?;
        self.write_register(REG_FRF_LSB, (frf & 0xFF) as u8)
    }

    fn write_register(&mut self, address: u8, value: u8) -> Result<(), KernelError> {
        self.bus.write(&[address | 0x80, value])
    }

    fn read_register(&mut self, address: u8) -> Result<u8, KernelError> {
        let mut frame = [address & 0x7F, 0x00];
        self.bus.transfer(&mut frame)?;
        Ok(frame[1])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockSx1276Spi {
    pub registers: [u8; 128],
    pub writes: Vec<(u8, u8), 96>,
    fifo: Vec<u8, 255>,
}

impl Default for MockSx1276Spi {
    fn default() -> Self {
        Self {
            registers: [0; 128],
            writes: Vec::new(),
            fifo: Vec::new(),
        }
    }
}

impl MockSx1276Spi {
    pub fn queue_rx_packet(
        &mut self,
        payload: &[u8],
        rssi_register: u8,
        snr_quarter_db: i8,
        crc_error_observed: bool,
    ) {
        self.fifo.clear();
        for byte in payload {
            let _ = self.fifo.push(*byte);
        }
        self.registers[REG_IRQ_FLAGS as usize] = IRQ_RX_DONE
            | if crc_error_observed {
                IRQ_PAYLOAD_CRC_ERROR
            } else {
                0
            };
        self.registers[REG_RX_NB_BYTES as usize] = payload.len() as u8;
        self.registers[REG_FIFO_RX_CURRENT_ADDR as usize] = 0x00;
        self.registers[REG_PKT_RSSI_VALUE as usize] = rssi_register;
        self.registers[REG_PKT_SNR_VALUE as usize] = snr_quarter_db as u8;
    }
}

impl Sx1276SpiBus for MockSx1276Spi {
    fn write(&mut self, bytes: &[u8]) -> Result<(), KernelError> {
        if bytes.len() < 2 {
            return Err(KernelError::Driver("SX1276 SPI write frame too short"));
        }
        let address = bytes[0] & 0x7F;
        let value = bytes[1];
        if address as usize >= self.registers.len() {
            return Err(KernelError::Driver("SX1276 register out of mock range"));
        }
        self.registers[address as usize] = value;
        self.writes
            .push((address, value))
            .map_err(|_| KernelError::Driver("SX1276 mock write log full"))
    }

    fn transfer(&mut self, bytes: &mut [u8]) -> Result<(), KernelError> {
        if bytes.len() < 2 {
            return Err(KernelError::Driver("SX1276 SPI transfer frame too short"));
        }
        let address = bytes[0] & 0x7F;
        bytes[1] = if address == REG_FIFO {
            if self.fifo.is_empty() {
                0
            } else {
                self.fifo.remove(0)
            }
        } else {
            *self
                .registers
                .get(address as usize)
                .ok_or(KernelError::Driver("SX1276 register out of mock range"))?
        };
        Ok(())
    }
}

fn validate_frequency(frequency_hz: u32) -> Result<(), KernelError> {
    if !(137_000_000..=1_020_000_000).contains(&frequency_hz) {
        return Err(KernelError::Driver(
            "SX1276 frequency outside supported range",
        ));
    }
    Ok(())
}

fn pa_config_for_dbm(dbm: i8) -> u8 {
    let level = (dbm.clamp(2, 17) - 2) as u8;
    0x80 | (level & 0x0F)
}

#[cfg(test)]
mod tests {
    use super::{
        MockSx1276Spi, PaThermalCwConfig, RawListenerConfig, Sx1276Driver, REG_MODEM_CONFIG_1,
        REG_MODEM_CONFIG_2, REG_OP_MODE,
    };

    #[test]
    fn raw_listener_uses_implicit_header_and_preserves_crc_error_packets() {
        let mut bus = MockSx1276Spi::default();
        bus.registers[REG_MODEM_CONFIG_1 as usize] = 0x72;
        bus.registers[REG_MODEM_CONFIG_2 as usize] = 0x74;
        bus.queue_rx_packet(&[0xDE, 0xAD, 0xBE, 0xEF], 70, -8, true);

        let mut driver = Sx1276Driver::<_, 4>::new(bus);
        driver
            .configure_raw_packet_listener(&RawListenerConfig {
                implicit_payload_len: 4,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            driver.bus().registers[REG_MODEM_CONFIG_1 as usize] & 0x01,
            0x01
        );
        assert_eq!(
            driver.bus().registers[REG_MODEM_CONFIG_2 as usize] & 0x04,
            0x00
        );

        let sample = driver.poll_raw_packet(123).unwrap().unwrap();
        assert_eq!(sample.bytes.as_slice(), &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(sample.crc_error_observed);
        assert_eq!(driver.raw_ring().len(), 1);
    }

    #[test]
    fn pa_thermal_cw_requires_lab_ack_and_is_bounded() {
        let bus = MockSx1276Spi::default();
        let mut driver = Sx1276Driver::<_, 2>::new(bus);

        assert!(driver
            .start_pa_thermal_cw(PaThermalCwConfig {
                frequency_hz: 868_100_000,
                pa_output_dbm: 17,
                max_duration_ms: 250,
                regulatory_acknowledged: false,
            })
            .is_err());

        let plan = driver
            .start_pa_thermal_cw(PaThermalCwConfig {
                frequency_hz: 868_100_000,
                pa_output_dbm: 17,
                max_duration_ms: 250,
                regulatory_acknowledged: true,
            })
            .unwrap();
        assert!(plan.continuous_wave_started);
        assert_eq!(driver.bus().registers[REG_OP_MODE as usize] & 0x07, 0x03);

        driver.stop_pa_thermal_cw().unwrap();
        assert_eq!(driver.bus().registers[REG_OP_MODE as usize] & 0x07, 0x01);
    }
}
