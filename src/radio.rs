use crate::error::KernelError;
use heapless::Vec;

pub const MAX_REGISTER_WRITES: usize = 8;
pub type WritePlan = Vec<RegisterWrite, MAX_REGISTER_WRITES>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterWrite {
    pub register: &'static str,
    pub value: u32,
}

pub trait RadioDriver {
    fn write_register(&mut self, write: RegisterWrite) -> Result<(), KernelError>;
    fn write_burst(&mut self, writes: &[RegisterWrite]) -> Result<(), KernelError> {
        for write in writes {
            self.write_register(*write)?;
        }
        Ok(())
    }
    fn enter_standby(&mut self) -> Result<(), KernelError>;
    fn commit(&mut self) -> Result<(), KernelError>;
}

#[derive(Debug, Default)]
pub struct MockRadio {
    pub standby_called: bool,
    pub committed: bool,
    pub writes: WritePlan,
}

impl RadioDriver for MockRadio {
    fn write_register(&mut self, write: RegisterWrite) -> Result<(), KernelError> {
        self.writes
            .push(write)
            .map_err(|_| KernelError::Driver("mock radio write buffer full"))
    }

    fn write_burst(&mut self, writes: &[RegisterWrite]) -> Result<(), KernelError> {
        self.writes
            .extend_from_slice(writes)
            .map_err(|_| KernelError::Driver("mock radio write buffer full"))
    }

    fn enter_standby(&mut self) -> Result<(), KernelError> {
        self.standby_called = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), KernelError> {
        self.committed = true;
        Ok(())
    }
}
