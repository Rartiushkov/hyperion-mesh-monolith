use core::ffi::c_char;

pub const RAMAN_DEVICE_ABI_VERSION_MAJOR: u16 = 1;
pub const RAMAN_DEVICE_ABI_VERSION_MINOR: u16 = 0;
pub const RAMAN_ARTIFACT_SLOT_MAGIC: u32 = 0x524D_534C;
pub const RAMAN_ARTIFACT_FLAG_FORCE_UPDATE: u32 = 0x0000_0001;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RamanArtifactSlotHeader {
    pub magic: u32,
    pub abi_version_major: u16,
    pub abi_version_minor: u16,
    pub version_id: u32,
    pub flags: u32,
    pub payload_len: u32,
    pub payload_crc32: u32,
}

impl RamanArtifactSlotHeader {
    pub const fn new(version_id: u32, flags: u32, payload_len: u32, payload_crc32: u32) -> Self {
        Self {
            magic: RAMAN_ARTIFACT_SLOT_MAGIC,
            abi_version_major: RAMAN_DEVICE_ABI_VERSION_MAJOR,
            abi_version_minor: RAMAN_DEVICE_ABI_VERSION_MINOR,
            version_id,
            flags,
            payload_len,
            payload_crc32,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RamanCSignalInput {
    pub minute: i32,
    pub noise_floor_dbm: i16,
    pub snr_db: f32,
    pub density: i32,
    pub latency_budget_ms: i32,
    pub battery_mv: i32,
    pub link_margin_db: f32,
    pub confidence: f32,
    pub drift_db: f32,
    pub rx_pdr: f32,
    pub error_rate: f32,
    pub path_stability: f32,
    pub memory_confidence: f32,
    pub worst_hop_reliability: f32,
    pub renegotiation_needed: u8,
    pub traffic_class: *const c_char,
    pub hardware: *const c_char,
    pub scenario: *const c_char,
    pub predicted: *const c_char,
    pub stream_id: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RamanCSignalOutput {
    pub status_code: i32,
    pub applied_at_us: u64,
    pub bandwidth_khz: u16,
    pub spreading_factor: u8,
    pub tx_power_dbm: i8,
    pub temporal_reused: u8,
    pub priority: [c_char; 32],
    pub profile_name: [c_char; 48],
    pub transport_priority: [c_char; 32],
    pub ris_mode: [c_char; 32],
}

impl Default for RamanCSignalOutput {
    fn default() -> Self {
        Self {
            status_code: 0,
            applied_at_us: 0,
            bandwidth_khz: 0,
            spreading_factor: 0,
            tx_power_dbm: 0,
            temporal_reused: 0,
            priority: [0; 32],
            profile_name: [0; 48],
            transport_priority: [0; 32],
            ris_mode: [0; 32],
        }
    }
}
