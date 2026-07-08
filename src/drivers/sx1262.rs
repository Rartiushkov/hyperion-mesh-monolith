use crate::error::KernelError;
use crate::radio::{RadioDriver, RegisterWrite, WritePlan};
use heapless::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct FieldLogEntry {
    pub timestamp_ms: u64,
    pub node_id: &'static str,
    pub hardware_id: &'static str,
    pub scenario_id: &'static str,
    pub rssi_dbm: i16,
    pub snr_db: i16,
    pub packet_delivery_ratio: f32,
    pub latency_ms: u16,
    pub payload_bytes: u16,
    pub node_density: u16,
    pub bandwidth_khz: u16,
}

pub trait LogSink {
    fn record(&mut self, entry: FieldLogEntry);
}

#[derive(Debug, Default)]
pub struct MemoryLogSink {
    pub entries: Vec<FieldLogEntry, 32>,
}

impl LogSink for MemoryLogSink {
    fn record(&mut self, entry: FieldLogEntry) {
        let _ = self.entries.push(entry);
    }
}

pub trait RegisterBus {
    fn enter_standby(&mut self) -> Result<(), KernelError>;
    fn write_opcode(&mut self, opcode: u8, value: u32) -> Result<(), KernelError>;
    fn write_opcode_burst(&mut self, writes: &[(u8, u32)]) -> Result<(), KernelError> {
        for (opcode, value) in writes {
            self.write_opcode(*opcode, *value)?;
        }
        Ok(())
    }
    fn commit(&mut self) -> Result<(), KernelError>;
}

#[derive(Debug, Default)]
pub struct MockSx1262Bus {
    pub standby_called: bool,
    pub committed: bool,
    pub opcode_writes: Vec<(u8, u32), 16>,
}

impl RegisterBus for MockSx1262Bus {
    fn enter_standby(&mut self) -> Result<(), KernelError> {
        self.standby_called = true;
        Ok(())
    }

    fn write_opcode(&mut self, opcode: u8, value: u32) -> Result<(), KernelError> {
        let _ = self.opcode_writes.push((opcode, value));
        Ok(())
    }

    fn commit(&mut self) -> Result<(), KernelError> {
        self.committed = true;
        Ok(())
    }
}

#[derive(Debug)]
pub struct Sx1262Driver<B, L> {
    node_id: &'static str,
    hardware_id: &'static str,
    bus: B,
    log_sink: L,
    writes: WritePlan,
}

impl<B, L> Sx1262Driver<B, L> {
    pub fn new(node_id: &'static str, hardware_id: &'static str, bus: B, log_sink: L) -> Self {
        Self {
            node_id,
            hardware_id,
            bus,
            log_sink,
            writes: WritePlan::new(),
        }
    }

    pub fn writes(&self) -> &[RegisterWrite] {
        &self.writes
    }

    pub fn bus(&self) -> &B {
        &self.bus
    }

    pub fn bus_mut(&mut self) -> &mut B {
        &mut self.bus
    }

    pub fn log_sink(&self) -> &L {
        &self.log_sink
    }
}

impl<B: RegisterBus, L: LogSink> Sx1262Driver<B, L> {
    pub fn record_link_sample(
        &mut self,
        timestamp_ms: u64,
        scenario_id: &'static str,
        rssi_dbm: i16,
        snr_db: i16,
        packet_delivery_ratio: f32,
        latency_ms: u16,
        payload_bytes: u16,
        node_density: u16,
        bandwidth_khz: u16,
    ) {
        self.log_sink.record(FieldLogEntry {
            timestamp_ms,
            node_id: self.node_id,
            hardware_id: self.hardware_id,
            scenario_id,
            rssi_dbm,
            snr_db,
            packet_delivery_ratio,
            latency_ms,
            payload_bytes,
            node_density,
            bandwidth_khz,
        });
    }
}

impl<B: RegisterBus, L: LogSink> RadioDriver for Sx1262Driver<B, L> {
    fn write_register(&mut self, write: RegisterWrite) -> Result<(), KernelError> {
        let opcode = opcode_for_register(write.register)?;
        self.bus.write_opcode(opcode, write.value)?;
        self.writes
            .push(write)
            .map_err(|_| KernelError::Driver("sx1262 driver write buffer full"))
    }

    fn write_burst(&mut self, writes: &[RegisterWrite]) -> Result<(), KernelError> {
        let mut opcodes = Vec::<(u8, u32), 16>::new();
        for write in writes {
            let opcode = opcode_for_register(write.register)?;
            opcodes
                .push((opcode, write.value))
                .map_err(|_| KernelError::Driver("sx1262 opcode burst buffer full"))?;
        }

        self.bus.write_opcode_burst(&opcodes)?;
        for write in writes {
            self.writes
                .push(*write)
                .map_err(|_| KernelError::Driver("sx1262 driver write buffer full"))?;
        }

        Ok(())
    }

    fn enter_standby(&mut self) -> Result<(), KernelError> {
        self.bus.enter_standby()
    }

    fn commit(&mut self) -> Result<(), KernelError> {
        self.bus.commit()
    }
}

fn opcode_for_register(register: &str) -> Result<u8, KernelError> {
    match register {
        "REG_OP_MODE" => Ok(0x01),
        "REG_FRF" => Ok(0x86),
        "REG_MODEM_CONFIG_1" => Ok(0x8B),
        "REG_MODEM_CONFIG_2" => Ok(0x8C),
        "REG_PACKET_CONFIG" => Ok(0x8D),
        "REG_PREAMBLE" => Ok(0x8E),
        "REG_SYNC_WORD" => Ok(0x74),
        "REG_PA_CONFIG" => Ok(0x95),
        _other => Err(KernelError::Driver("unsupported SX1262 register mapping")),
    }
}

#[cfg(test)]
mod tests {
    use super::{LogSink, MemoryLogSink, MockSx1262Bus, RegisterBus, Sx1262Driver};
    use crate::radio::{RadioDriver, RegisterWrite};
    use crate::{Context, MorphicKernel};
    use heapless::Vec;

    #[derive(Debug, Default)]
    struct OrderedBus {
        events: Vec<&'static str, 16>,
        opcode_writes: Vec<(u8, u32), 16>,
    }

    impl RegisterBus for OrderedBus {
        fn enter_standby(&mut self) -> Result<(), crate::KernelError> {
            let _ = self.events.push("standby");
            Ok(())
        }

        fn write_opcode(&mut self, opcode: u8, value: u32) -> Result<(), crate::KernelError> {
            let _ = self.events.push("write");
            let _ = self.opcode_writes.push((opcode, value));
            Ok(())
        }

        fn commit(&mut self) -> Result<(), crate::KernelError> {
            let _ = self.events.push("commit");
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct NullLogSink;

    impl LogSink for NullLogSink {
        fn record(&mut self, _entry: super::FieldLogEntry) {}
    }

    #[test]
    fn sx1262_driver_maps_registers_and_records_field_logs() {
        let bus = MockSx1262Bus::default();
        let sink = MemoryLogSink::default();
        let mut driver = Sx1262Driver::new("RDN-GW-001", "esp32_sx1262_gateway", bus, sink);

        driver.enter_standby().unwrap();
        driver
            .write_register(RegisterWrite {
                register: "REG_FRF",
                value: 868_100_000,
            })
            .unwrap();
        driver.record_link_sample(
            1_743_327_000,
            "industrial_shift",
            -94,
            8,
            0.98,
            162,
            32,
            16,
            125,
        );
        driver.commit().unwrap();

        assert_eq!(driver.writes().len(), 1);
        assert_eq!(driver.bus().opcode_writes[0], (0x86, 868_100_000));
        assert_eq!(driver.log_sink().entries.len(), 1);
        assert_eq!(
            driver.log_sink().entries[0].hardware_id,
            "esp32_sx1262_gateway"
        );
    }

    #[test]
    fn sx1262_driver_preserves_standby_write_commit_sequence() {
        let kernel = MorphicKernel::new();
        let current = crate::PhyProfile::lora_default();
        let target = kernel.planner().generate_phy(&Context::constrained());
        let bus = OrderedBus::default();
        let sink = NullLogSink;
        let mut driver = Sx1262Driver::new("RDN-RLY-001", "sx1262_relay", bus, sink);

        let writes = kernel
            .apply_profile(&mut driver, &current, &target)
            .unwrap();

        assert!(!writes.is_empty());
        assert_eq!(driver.bus().events.first(), Some(&"standby"));
        assert_eq!(driver.bus().events.last(), Some(&"commit"));
        assert!(driver
            .bus()
            .events
            .iter()
            .skip(1)
            .take(writes.len())
            .all(|event| *event == "write"));
        assert_eq!(driver.bus().opcode_writes.len(), writes.len());
    }
}
