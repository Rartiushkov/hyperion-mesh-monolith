#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    AuthTagMismatch,
    ReplayOrStaleNonce,
    NonceOutOfWindow,
    ResyncSequenceGap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureFrame32 {
    pub nonce: u32,
    pub wire: u32,
    pub tag: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayWindow {
    pub expected_nonce: u32,
    pub max_future_skew_packets: u32,
}

impl ReplayWindow {
    pub const fn new(expected_nonce: u32, max_future_skew_packets: u32) -> Self {
        Self {
            expected_nonce,
            max_future_skew_packets,
        }
    }
}

impl Default for SecureFrame32 {
    fn default() -> Self {
        Self {
            nonce: 0,
            wire: 0,
            tag: 0,
        }
    }
}

impl SecureFrame32 {
    #[inline]
    pub fn encode(payload: u16, nonce: u32, session: u64) -> Self {
        let wire = u32::from(payload) ^ frame_mask(session, nonce);
        let tag = frame_tag(session, nonce, wire);

        Self { nonce, wire, tag }
    }

    #[inline]
    pub fn encode_in_place(&mut self, payload: u16, nonce: u32, session: u64) {
        self.nonce = nonce;
        self.wire = u32::from(payload) ^ frame_mask(session, nonce);
        self.tag = frame_tag(session, nonce, self.wire);
    }

    #[inline]
    pub fn decode(&self, session: u64) -> Result<u16, PacketError> {
        if self.tag != frame_tag(session, self.nonce, self.wire) {
            return Err(PacketError::AuthTagMismatch);
        }

        Ok((self.wire ^ frame_mask(session, self.nonce)) as u16)
    }
}

#[inline]
pub fn accept_frame(
    frame: &SecureFrame32,
    session: u64,
    window: &mut ReplayWindow,
) -> Result<u16, PacketError> {
    if frame.nonce < window.expected_nonce {
        return Err(PacketError::ReplayOrStaleNonce);
    }

    if frame.nonce > window.expected_nonce + window.max_future_skew_packets {
        return Err(PacketError::NonceOutOfWindow);
    }

    let payload = frame.decode(session)?;
    window.expected_nonce = frame.nonce.saturating_add(1);
    Ok(payload)
}

#[inline]
pub fn resync_window(
    sync_a: &SecureFrame32,
    sync_b: &SecureFrame32,
    session: u64,
    window: &mut ReplayWindow,
) -> Result<(), PacketError> {
    if sync_b.nonce != sync_a.nonce.saturating_add(1) {
        return Err(PacketError::ResyncSequenceGap);
    }

    let _ = sync_a.decode(session)?;
    let _ = sync_b.decode(session)?;
    window.expected_nonce = sync_b.nonce.saturating_add(1);
    Ok(())
}

#[inline]
pub const fn frame_mask(session: u64, nonce: u32) -> u32 {
    let mixed = splitmix64(session ^ ((nonce as u64) << 16) ^ 0x9E37_79B9_7F4A_7C15);
    (mixed as u32) ^ ((mixed >> 32) as u32)
}

#[inline]
pub const fn frame_tag(session: u64, nonce: u32, wire: u32) -> u64 {
    splitmix64(session.rotate_left((nonce & 31) + 1) ^ ((nonce as u64) << 32) ^ wire as u64)
}

#[inline]
const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::{accept_frame, resync_window, PacketError, ReplayWindow, SecureFrame32};

    #[test]
    fn secure_frame_round_trip_and_resync() {
        let session = 0xA4D1_55E7_1020_33CCu64;
        let frame = SecureFrame32::encode(0x0101, 42, session);
        let mut window = ReplayWindow::new(42, 8);

        assert_eq!(accept_frame(&frame, session, &mut window).unwrap(), 0x0101);
        assert_eq!(window.expected_nonce, 43);

        let sync_a = SecureFrame32::encode(0xAAAA, 100, session);
        let sync_b = SecureFrame32::encode(0x5555, 101, session);
        resync_window(&sync_a, &sync_b, session, &mut window).unwrap();
        assert_eq!(window.expected_nonce, 102);
    }

    #[test]
    fn poisoned_tag_is_rejected() {
        let session = 0x55AA_10FE_C011_EC7Du64;
        let mut frame = SecureFrame32::encode(0x0101, 7, session);
        frame.tag ^= 0x10;

        assert_eq!(
            frame.decode(session).unwrap_err(),
            PacketError::AuthTagMismatch
        );
    }
}
