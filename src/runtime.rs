use crate::{
    KernelError, PhyProfile, RamanArtifactError, RamanExecutorState, RamanMirrorExecutor,
    RamanRuntimeArtifact, RamanRuntimeContext, RamanRuntimeResult, RamanRuntimeRis,
};

pub trait RamanClock {
    fn monotonic_us(&self) -> u64;
}

pub trait RamanControlPlane {
    fn apply_phy(&mut self, profile: &PhyProfile) -> Result<(), KernelError>;
    fn set_transport_priority(&mut self, priority: Option<&str>) -> Result<(), KernelError>;
    fn set_ris_mode(&mut self, ris: Option<&RamanRuntimeRis>) -> Result<(), KernelError>;
}

pub trait RamanPlatform: RamanClock + RamanControlPlane {}

impl<T> RamanPlatform for T where T: RamanClock + RamanControlPlane {}

#[derive(Debug, Clone, PartialEq)]
pub struct RamanHostReport {
    pub decision: RamanRuntimeResult,
    pub applied_at_us: u64,
}

pub struct RamanRuntimeHost<P> {
    platform: P,
    executor: RamanMirrorExecutor,
    state: RamanExecutorState,
}

impl<P> RamanRuntimeHost<P>
where
    P: RamanPlatform,
{
    pub fn from_artifact(
        platform: P,
        artifact: &RamanRuntimeArtifact,
    ) -> Result<Self, RamanArtifactError> {
        Ok(Self {
            platform,
            executor: RamanMirrorExecutor::from_artifact(artifact)?,
            state: RamanExecutorState::new(),
        })
    }

    pub fn process_signal(
        &mut self,
        stream_id: &str,
        context: &RamanRuntimeContext,
    ) -> Result<RamanHostReport, RamanArtifactError> {
        let decision = self.executor.execute(stream_id, context, &mut self.state)?;
        self.platform
            .apply_phy(&decision.snapshot.phy)
            .map_err(host_error)?;
        self.platform
            .set_transport_priority(decision.snapshot.transport_priority.as_deref())
            .map_err(host_error)?;
        self.platform
            .set_ris_mode(decision.snapshot.ris.as_ref())
            .map_err(host_error)?;
        Ok(RamanHostReport {
            decision,
            applied_at_us: self.platform.monotonic_us(),
        })
    }

    pub fn platform(&self) -> &P {
        &self.platform
    }

    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }
}

#[derive(Debug)]
pub struct NoopRamanPlatform {
    started_at: std::time::Instant,
    pub last_phy: Option<PhyProfile>,
    pub last_transport_priority: Option<String>,
    pub last_ris_mode: Option<String>,
}

impl Default for NoopRamanPlatform {
    fn default() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            last_phy: None,
            last_transport_priority: None,
            last_ris_mode: None,
        }
    }
}

impl RamanClock for NoopRamanPlatform {
    fn monotonic_us(&self) -> u64 {
        self.started_at.elapsed().as_micros() as u64
    }
}

impl RamanControlPlane for NoopRamanPlatform {
    fn apply_phy(&mut self, profile: &PhyProfile) -> Result<(), KernelError> {
        self.last_phy = Some(*profile);
        Ok(())
    }

    fn set_transport_priority(&mut self, priority: Option<&str>) -> Result<(), KernelError> {
        self.last_transport_priority = priority.map(str::to_string);
        Ok(())
    }

    fn set_ris_mode(&mut self, ris: Option<&RamanRuntimeRis>) -> Result<(), KernelError> {
        self.last_ris_mode = ris.map(|value| value.mode.clone());
        Ok(())
    }
}

fn host_error(error: KernelError) -> RamanArtifactError {
    RamanArtifactError::RuntimeParse(format!("Raman host apply failed: {error}"))
}
