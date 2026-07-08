pub const TELEMETRY_FRAME_LEN: usize = 24;
pub const TELEMETRY_MAGIC: u8 = 0xA7;
pub const TELEMETRY_TERMINATOR: u8 = 0x5A;

const OFFSET_MAGIC: usize = 0;
const OFFSET_FRAME_TYPE: usize = 1;
const OFFSET_SOURCE_ID: usize = 2;
const OFFSET_SEQUENCE: usize = 4;
const OFFSET_PAYLOAD_LEN: usize = 8;
const OFFSET_UPTIME_MS: usize = 10;
const OFFSET_LINK_QUALITY: usize = 14;
const OFFSET_PAYLOAD: usize = 16;
const PAYLOAD_INLINE_LEN: usize = 6;
const OFFSET_CHECKSUM: usize = 22;
const OFFSET_TERMINATOR: usize = 23;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryFrameFields {
    pub frame_type: u8,
    pub source_id: u16,
    pub sequence_number: u32,
    pub uptime_ms: u32,
    pub link_quality_permille: u16,
    pub payload: [u8; PAYLOAD_INLINE_LEN],
    pub payload_len: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryFrame {
    bytes: [u8; TELEMETRY_FRAME_LEN],
}

impl TelemetryFrame {
    pub fn new(fields: TelemetryFrameFields) -> Self {
        let mut frame = Self {
            bytes: [0; TELEMETRY_FRAME_LEN],
        };
        frame.bytes[OFFSET_MAGIC] = TELEMETRY_MAGIC;
        frame.bytes[OFFSET_FRAME_TYPE] = fields.frame_type;
        frame.write_source_id_unchecked(fields.source_id);
        frame.write_sequence_unchecked(fields.sequence_number);
        frame.bytes[OFFSET_PAYLOAD_LEN..OFFSET_PAYLOAD_LEN + 2]
            .copy_from_slice(&fields.payload_len.to_le_bytes());
        frame.bytes[OFFSET_UPTIME_MS..OFFSET_UPTIME_MS + 4]
            .copy_from_slice(&fields.uptime_ms.to_le_bytes());
        frame.bytes[OFFSET_LINK_QUALITY..OFFSET_LINK_QUALITY + 2]
            .copy_from_slice(&fields.link_quality_permille.to_le_bytes());
        frame.bytes[OFFSET_PAYLOAD..OFFSET_PAYLOAD + PAYLOAD_INLINE_LEN]
            .copy_from_slice(&fields.payload);
        frame.bytes[OFFSET_TERMINATOR] = TELEMETRY_TERMINATOR;
        frame.refresh_checksum();
        frame
    }

    pub fn as_bytes(&self) -> &[u8; TELEMETRY_FRAME_LEN] {
        &self.bytes
    }

    pub fn into_bytes(self) -> [u8; TELEMETRY_FRAME_LEN] {
        self.bytes
    }

    pub fn source_id(&self) -> u16 {
        u16::from_le_bytes([
            self.bytes[OFFSET_SOURCE_ID],
            self.bytes[OFFSET_SOURCE_ID + 1],
        ])
    }

    pub fn sequence_number(&self) -> u32 {
        u32::from_le_bytes([
            self.bytes[OFFSET_SEQUENCE],
            self.bytes[OFFSET_SEQUENCE + 1],
            self.bytes[OFFSET_SEQUENCE + 2],
            self.bytes[OFFSET_SEQUENCE + 3],
        ])
    }

    pub fn overwrite_source_id(&mut self, source_id: u16, recompute_checksum: bool) {
        self.write_source_id_unchecked(source_id);
        if recompute_checksum {
            self.refresh_checksum();
        }
    }

    pub fn overwrite_sequence_number(&mut self, sequence_number: u32, recompute_checksum: bool) {
        self.write_sequence_unchecked(sequence_number);
        if recompute_checksum {
            self.refresh_checksum();
        }
    }

    pub fn checksum(&self) -> u8 {
        self.bytes[OFFSET_CHECKSUM]
    }

    pub fn checksum_valid(&self) -> bool {
        self.checksum() == compute_checksum(&self.bytes)
    }

    fn write_source_id_unchecked(&mut self, source_id: u16) {
        self.bytes[OFFSET_SOURCE_ID..OFFSET_SOURCE_ID + 2]
            .copy_from_slice(&source_id.to_le_bytes());
    }

    fn write_sequence_unchecked(&mut self, sequence_number: u32) {
        self.bytes[OFFSET_SEQUENCE..OFFSET_SEQUENCE + 4]
            .copy_from_slice(&sequence_number.to_le_bytes());
    }

    fn refresh_checksum(&mut self) {
        self.bytes[OFFSET_CHECKSUM] = 0;
        self.bytes[OFFSET_CHECKSUM] = compute_checksum(&self.bytes);
    }
}

fn compute_checksum(bytes: &[u8; TELEMETRY_FRAME_LEN]) -> u8 {
    bytes
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != OFFSET_CHECKSUM)
        .fold(0x5Du8, |acc, (_, byte)| acc.rotate_left(1) ^ *byte)
}

#[cfg(test)]
mod tests {
    use super::{TelemetryFrame, TelemetryFrameFields, TELEMETRY_MAGIC, TELEMETRY_TERMINATOR};

    #[test]
    fn telemetry_frame_manual_overwrites_can_preserve_or_break_checksum() {
        let mut frame = TelemetryFrame::new(TelemetryFrameFields {
            frame_type: 0x11,
            source_id: 0x1234,
            sequence_number: 7,
            uptime_ms: 42_000,
            link_quality_permille: 991,
            payload: [1, 2, 3, 4, 5, 6],
            payload_len: 6,
        });

        assert_eq!(frame.as_bytes()[0], TELEMETRY_MAGIC);
        assert_eq!(frame.as_bytes()[23], TELEMETRY_TERMINATOR);
        assert!(frame.checksum_valid());

        frame.overwrite_source_id(0xBEEF, true);
        frame.overwrite_sequence_number(u32::MAX, true);
        assert_eq!(frame.source_id(), 0xBEEF);
        assert_eq!(frame.sequence_number(), u32::MAX);
        assert!(frame.checksum_valid());

        frame.overwrite_sequence_number(1, false);
        assert_eq!(frame.sequence_number(), 1);
        assert!(!frame.checksum_valid());
    }
}
