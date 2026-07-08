use pretty_assertions::assert_eq;
use proptest::prelude::*;
use radnet_morphic_kernel::{
    accept_frame, resync_window, Bandwidth, CodingRate, Context, MockRadio, MorphicKernel,
    PacketError, ReplayWindow, SecureFrame32, SpreadingFactor,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

fn liar_positions_count<const N: usize>(seed: u64) -> [usize; N] {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut result = [0usize; N];
    let mut count = 0usize;

    while count < result.len() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let candidate = (state % 12) as usize;

        if !result[..count].contains(&candidate) {
            result[count] = candidate;
            count += 1;
        }
    }

    result
}

fn liar_positions(seed: u64) -> [usize; 4] {
    liar_positions_count::<4>(seed)
}

fn packet_error_label(error: PacketError) -> &'static str {
    match error {
        PacketError::AuthTagMismatch => "auth_tag_mismatch",
        PacketError::ReplayOrStaleNonce => "replay_or_stale_nonce",
        PacketError::NonceOutOfWindow => "nonce_out_of_window",
        PacketError::ResyncSequenceGap => "resync_sequence_gap",
    }
}

fn singularity_context(hop: usize, interference_db: u8, doppler_shift_khz: i16) -> Context {
    let harsh_hop = hop % 3 == 2;
    let harsh_channel = interference_db >= 15 || doppler_shift_khz.unsigned_abs() >= 80;

    if harsh_hop || harsh_channel {
        Context {
            noise_floor_dbm: -109 + hop as i16,
            snr_db: -8 + (hop % 2) as i8,
            battery_mv: 3_340,
            latency_budget_ms: 220,
            link_margin_db: 3,
        }
    } else {
        Context {
            noise_floor_dbm: -126,
            snr_db: 10,
            battery_mv: 3_700,
            latency_budget_ms: 45,
            link_margin_db: 16,
        }
    }
}

fn run_singularity_case(
    payload: u16,
    session: u64,
    base_nonce: u32,
    clock_skew_secs: u8,
    interference_db: u8,
    doppler_shift_khz: i16,
    byzantine_seed: u64,
) -> Result<(), &'static str> {
    let kernel = MorphicKernel::new();
    let liars = liar_positions(byzantine_seed);
    let skew_packets = u32::from(clock_skew_secs) * 2;
    let command_nonce = base_nonce.saturating_add(skew_packets).saturating_add(2);
    let mut window = ReplayWindow::new(base_nonce, 8);

    if command_nonce > window.expected_nonce + window.max_future_skew_packets {
        let sync_a = SecureFrame32::encode(0xAAAA, base_nonce + skew_packets, session);
        let sync_b = SecureFrame32::encode(0x5555, base_nonce + skew_packets + 1, session);
        resync_window(&sync_a, &sync_b, session, &mut window).map_err(packet_error_label)?;
    }

    let mut current_preset = radnet_morphic_kernel::ProfilePreset::Default;
    let mut current_frame = SecureFrame32::default();
    current_frame.encode_in_place(payload, command_nonce, session);

    for hop in 0..12 {
        let context = singularity_context(hop, interference_db, doppler_shift_khz);
        let target_preset = kernel.planner().select_preset(&context);

        let mut radio = MockRadio::default();
        let writes = kernel
            .apply_preset(&mut radio, current_preset, target_preset)
            .map_err(|_| "apply_failed")?;
        if !radio.standby_called || !radio.committed {
            return Err("invalid_radio_transition");
        }
        if current_preset != target_preset && writes.is_empty() {
            return Err("missing_morphic_switch");
        }
        current_preset = target_preset;

        if liars.contains(&hop) {
            let spoof = SecureFrame32 {
                nonce: current_frame.nonce + 1,
                wire: current_frame.wire ^ (0x1000 + hop as u32),
                tag: current_frame.tag ^ (0xDEAD_BEEF_u64 + hop as u64),
            };
            if accept_frame(&spoof, session, &mut window).is_ok() {
                return Err("spoof_accepted");
            }
        }

        let decoded =
            accept_frame(&current_frame, session, &mut window).map_err(packet_error_label)?;
        if decoded != payload {
            return Err("payload_corrupted");
        }

        if hop < 11 {
            current_frame.encode_in_place(payload, current_frame.nonce + 1, session);
        }
    }

    let final_payload = current_frame.decode(session).map_err(packet_error_label)?;
    if final_payload != payload {
        return Err("bit_exact_restore_failed");
    }

    Ok(())
}

fn ghost_protocol_context() -> Context {
    Context {
        noise_floor_dbm: -140,
        snr_db: -40,
        battery_mv: 3_280,
        latency_budget_ms: 500,
        link_margin_db: -24,
    }
}

#[derive(Debug, Clone, Copy)]
struct QuantumChaosEnv {
    bit_flip_rate: f32,
    byzantine_fraction: f32,
    observer_collapse: bool,
    stateless_hops: bool,
    clock_skew_packets: u32,
}

impl QuantumChaosEnv {
    fn new() -> Self {
        Self {
            bit_flip_rate: 0.0,
            byzantine_fraction: 0.0,
            observer_collapse: false,
            stateless_hops: false,
            clock_skew_packets: 0,
        }
    }

    fn bit_flip_storm(mut self, rate: f32) -> Self {
        self.bit_flip_rate = rate;
        self
    }

    fn byzantine_hell(mut self, fraction: f32) -> Self {
        self.byzantine_fraction = fraction;
        self
    }

    fn observer_collapse(mut self, enabled: bool) -> Self {
        self.observer_collapse = enabled;
        self
    }

    fn stateless_hops(mut self, enabled: bool) -> Self {
        self.stateless_hops = enabled;
        self
    }

    fn time_travel_skew_packets(mut self, packets: u32) -> Self {
        self.clock_skew_packets = packets;
        self
    }
}

fn quantum_liar_positions(seed: u64, fraction: f32) -> [usize; 8] {
    let _ = fraction;
    liar_positions_count::<8>(seed)
}

fn observe_frame(frame: &SecureFrame32, observer_seed: u64) -> u64 {
    let fold = u64::from(frame.nonce) << 32 | u64::from(frame.wire);
    fold.rotate_left((observer_seed as u32 & 31) + 1) ^ frame.tag
}

fn bit_flip_u32(mut value: u32, seed: u64, rate: f32) -> u32 {
    let threshold = (rate * 100.0) as u64;
    for bit in 0..32 {
        let selector = seed.rotate_left(bit as u32) ^ (u64::from(bit as u32) << 17);
        if selector % 100 < threshold {
            value ^= 1u32 << bit;
        }
    }
    value
}

fn bit_flip_u64(mut value: u64, seed: u64, rate: f32) -> u64 {
    let threshold = (rate * 100.0) as u64;
    for bit in 0..64 {
        let selector = seed.rotate_left(bit as u32) ^ ((bit as u64) << 23);
        if selector % 100 < threshold {
            value ^= 1u64 << bit;
        }
    }
    value
}

fn quantum_corrupt(
    frame: &SecureFrame32,
    env: QuantumChaosEnv,
    seed: u64,
    observer_measurement: u64,
) -> SecureFrame32 {
    let observer_mix = if env.observer_collapse {
        observer_measurement
    } else {
        0
    };

    SecureFrame32 {
        nonce: frame.nonce,
        wire: bit_flip_u32(frame.wire, seed ^ observer_mix, env.bit_flip_rate),
        tag: bit_flip_u64(
            frame.tag,
            seed.rotate_left(13) ^ observer_mix.rotate_left(7),
            env.bit_flip_rate,
        ),
    }
}

fn run_quantum_ghost_case(
    payload: u16,
    session: u64,
    base_nonce: u32,
    env: QuantumChaosEnv,
    bitflip_seed: u64,
    observer_seed: u64,
    byzantine_seed: u64,
) -> Result<(), &'static str> {
    let kernel = MorphicKernel::new();
    let liars = quantum_liar_positions(byzantine_seed, env.byzantine_fraction);
    let mut window = ReplayWindow::new(base_nonce, 8);
    let command_nonce = base_nonce
        .saturating_add(env.clock_skew_packets)
        .saturating_add(2);

    if command_nonce > window.expected_nonce + window.max_future_skew_packets {
        let sync_a = SecureFrame32::encode(0xAAAA, command_nonce - 2, session);
        let sync_b = SecureFrame32::encode(0x5555, command_nonce - 1, session);
        resync_window(&sync_a, &sync_b, session, &mut window).map_err(packet_error_label)?;
    }

    let mut current_preset = radnet_morphic_kernel::ProfilePreset::Default;
    let mut nonce_cursor = command_nonce;

    for hop in 0..12 {
        let context = singularity_context(hop, 16, 100);
        let target_preset = kernel.planner().select_preset(&context);

        let mut radio = MockRadio::default();
        let writes = kernel
            .apply_preset(&mut radio, current_preset, target_preset)
            .map_err(|_| "apply_failed")?;
        if !radio.standby_called || !radio.committed {
            return Err("invalid_radio_transition");
        }
        if current_preset != target_preset && writes.is_empty() {
            return Err("missing_morphic_switch");
        }
        current_preset = target_preset;

        let honest_frame = SecureFrame32::encode(payload, nonce_cursor, session);
        let observation = observe_frame(&honest_frame, observer_seed ^ u64::from(hop as u32));
        let corrupted_frame = quantum_corrupt(
            &honest_frame,
            env,
            bitflip_seed ^ (u64::from(hop as u32) << 8),
            observation,
        );

        if corrupted_frame != honest_frame
            && accept_frame(&corrupted_frame, session, &mut window).is_ok()
        {
            return Err("bitflip_corruption_accepted");
        }

        if liars.contains(&hop) {
            let spoof = SecureFrame32 {
                nonce: nonce_cursor,
                wire: honest_frame.wire ^ (0x9B00 + hop as u32),
                tag: honest_frame.tag ^ (0xC0DE_CAFE_u64 + hop as u64),
            };
            if accept_frame(&spoof, session, &mut window).is_ok() {
                return Err("spoof_accepted");
            }
        }

        let decoded =
            accept_frame(&honest_frame, session, &mut window).map_err(packet_error_label)?;
        if decoded != payload {
            return Err("payload_corrupted");
        }

        if env.stateless_hops && hop < 11 {
            nonce_cursor = nonce_cursor.saturating_add(1);
        }
    }

    Ok(())
}

fn schrodinger_route_context(route: usize, hop: usize) -> Context {
    match route {
        0 => singularity_context(hop, 16, 88),
        1 => singularity_context(hop, 15, 92),
        _ => singularity_context(hop, 14, 84),
    }
}

fn corrupt_wire_and_tag(
    frame: &SecureFrame32,
    seed: u64,
    wire_rate: f32,
    tag_rate: f32,
) -> SecureFrame32 {
    SecureFrame32 {
        nonce: frame.nonce,
        wire: bit_flip_u32(frame.wire, seed, wire_rate),
        tag: bit_flip_u64(frame.tag, seed.rotate_left(11), tag_rate),
    }
}

fn quantum_collapse_decode(candidates: &[(SecureFrame32, u64); 3]) -> Result<u16, &'static str> {
    let mut resolved: Option<u16> = None;

    for (frame, session) in candidates {
        if let Ok(payload) = frame.decode(*session) {
            match resolved {
                Some(previous) if previous != payload => return Err("collapse_conflict"),
                Some(_) => {}
                None => resolved = Some(payload),
            }
        }
    }

    resolved.ok_or("collapse_no_truth")
}

fn run_schrodinger_collapse_case(
    payload: u16,
    base_nonce: u32,
    base_session: u64,
    route_seed: u64,
    observer_seed: u64,
) -> Result<(), &'static str> {
    let kernel = MorphicKernel::new();
    let nonce = base_nonce.saturating_add(12);
    let mut route_presets = [radnet_morphic_kernel::ProfilePreset::Default; 3];

    for hop in 0..12 {
        for (route, current_preset) in route_presets.iter_mut().enumerate() {
            let target_preset = kernel
                .planner()
                .select_preset(&schrodinger_route_context(route, hop));
            let mut radio = MockRadio::default();
            let writes = kernel
                .apply_preset(&mut radio, *current_preset, target_preset)
                .map_err(|_| "apply_failed")?;
            if !radio.standby_called || !radio.committed {
                return Err("invalid_radio_transition");
            }
            if *current_preset != target_preset && writes.is_empty() {
                return Err("missing_morphic_switch");
            }
            *current_preset = target_preset;
        }
    }

    let route_sessions = [
        base_session ^ 0xA1A1_1111_0000_0001u64,
        base_session ^ 0xB2B2_2222_0000_0002u64,
        base_session ^ 0xC3C3_3333_0000_0003u64,
    ];

    let route_frames = [
        SecureFrame32::encode(payload, nonce, route_sessions[0]),
        SecureFrame32::encode(payload, nonce, route_sessions[1]),
        SecureFrame32::encode(payload, nonce, route_sessions[2]),
    ];

    let observer_mix_a = observe_frame(&route_frames[0], observer_seed);
    let observer_mix_b = observe_frame(&route_frames[1], observer_seed.rotate_left(7));
    let _observer_mix_c = observe_frame(&route_frames[2], observer_seed.rotate_left(13));

    let candidates = [
        (
            corrupt_wire_and_tag(&route_frames[0], route_seed ^ observer_mix_a, 0.15, 0.12),
            route_sessions[0],
        ),
        (
            corrupt_wire_and_tag(
                &route_frames[1],
                route_seed.rotate_left(9) ^ observer_mix_b,
                0.20,
                0.16,
            ),
            route_sessions[1],
        ),
        (route_frames[2], route_sessions[2]),
    ];

    let recovered = quantum_collapse_decode(&candidates)?;
    if recovered != payload {
        return Err("collapse_payload_mismatch");
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct EntangledShard {
    nonce: u32,
    session: u64,
    payload_mask: u16,
    payload_bits: u16,
    tag_mask: u64,
    tag_bits: u64,
}

const ENTANGLED_PAYLOAD_MASKS: [u16; 3] = [
    0x07FF, // bits 0..10
    0xFFE0, // bits 5..15
    0xF83F, // bits 0..5 and 11..15
];

const ENTANGLED_TAG_MASKS: [u64; 3] = [
    0x0000_0000_0000_7FFF,
    0x0000_0000_3FFF_8000,
    0x0000_1FFF_C000_0000,
];

fn build_entangled_shard(payload: u16, nonce: u32, session: u64, route: usize) -> EntangledShard {
    let frame = SecureFrame32::encode(payload, nonce, session);
    EntangledShard {
        nonce,
        session,
        payload_mask: ENTANGLED_PAYLOAD_MASKS[route],
        payload_bits: payload & ENTANGLED_PAYLOAD_MASKS[route],
        tag_mask: ENTANGLED_TAG_MASKS[route],
        tag_bits: frame.tag & ENTANGLED_TAG_MASKS[route],
    }
}

fn poison_entangled_shard(mut shard: EntangledShard, poison_seed: u64) -> EntangledShard {
    let payload_flip = ((poison_seed as u16) | 1) & shard.payload_mask;
    let tag_flip = (poison_seed.rotate_left(17) | 1) & shard.tag_mask;
    shard.payload_bits ^= payload_flip;
    shard.tag_bits ^= tag_flip;
    shard
}

fn quantum_reconstruct_entangled(shards: &[EntangledShard; 3]) -> Result<u16, &'static str> {
    let mut resolved: Option<u16> = None;

    for &(left, right) in &[(0usize, 1usize), (0usize, 2usize), (1usize, 2usize)] {
        let left_shard = shards[left];
        let right_shard = shards[right];
        let overlap = left_shard.payload_mask & right_shard.payload_mask;

        if ((left_shard.payload_bits ^ right_shard.payload_bits) & overlap) != 0 {
            continue;
        }

        let candidate = left_shard.payload_bits | right_shard.payload_bits;
        if (left_shard.payload_mask | right_shard.payload_mask) != u16::MAX {
            continue;
        }

        let left_expected = SecureFrame32::encode(candidate, left_shard.nonce, left_shard.session);
        let right_expected =
            SecureFrame32::encode(candidate, right_shard.nonce, right_shard.session);

        let left_valid = (left_expected.tag & left_shard.tag_mask) == left_shard.tag_bits;
        let right_valid = (right_expected.tag & right_shard.tag_mask) == right_shard.tag_bits;
        if !(left_valid && right_valid) {
            continue;
        }

        match resolved {
            Some(previous) if previous != candidate => return Err("collapse_conflict"),
            Some(_) => {}
            None => resolved = Some(candidate),
        }
    }

    resolved.ok_or("insufficient_consensus")
}

fn run_quantum_entanglement_case(
    payload: u16,
    base_nonce: u32,
    session: u64,
    poison_seed: u64,
) -> Result<(), &'static str> {
    let kernel = MorphicKernel::new();
    let nonce = base_nonce.saturating_add(12);
    let route_sessions = [
        session ^ 0xD1D1_0000_0000_0001u64,
        session ^ 0xE2E2_0000_0000_0002u64,
        session ^ 0xF3F3_0000_0000_0003u64,
    ];
    let mut route_presets = [radnet_morphic_kernel::ProfilePreset::Default; 3];

    for hop in 0..12 {
        for (route, current_preset) in route_presets.iter_mut().enumerate() {
            let target_preset = kernel
                .planner()
                .select_preset(&schrodinger_route_context(route, hop));
            let mut radio = MockRadio::default();
            let writes = kernel
                .apply_preset(&mut radio, *current_preset, target_preset)
                .map_err(|_| "apply_failed")?;
            if !radio.standby_called || !radio.committed {
                return Err("invalid_radio_transition");
            }
            if *current_preset != target_preset && writes.is_empty() {
                return Err("missing_morphic_switch");
            }
            *current_preset = target_preset;
        }
    }

    let poisoned_route = (poison_seed % 3) as usize;
    let mut shards = [
        build_entangled_shard(payload, nonce, route_sessions[0], 0),
        build_entangled_shard(payload, nonce, route_sessions[1], 1),
        build_entangled_shard(payload, nonce, route_sessions[2], 2),
    ];
    shards[poisoned_route] =
        poison_entangled_shard(shards[poisoned_route], poison_seed.rotate_left(11));

    let recovered = quantum_reconstruct_entangled(&shards)?;
    if recovered != payload {
        return Err("entanglement_payload_mismatch");
    }

    Ok(())
}

fn build_eternal_origin_shards(
    payload: u16,
    nonce: u32,
    session: u64,
    poison_seed: u64,
) -> [EntangledShard; 3] {
    let base_frame = SecureFrame32::encode(payload, nonce, session);
    let surviving_mask = 0x0001u16;
    let surviving_bit = payload & surviving_mask;
    let tag_mask = 0x0000_0000_0000_0001u64;
    let lie_mask = if poison_seed & 1 == 0 {
        surviving_mask
    } else {
        0
    };
    let lie_tag = if poison_seed.rotate_left(5) & 1 == 0 {
        tag_mask
    } else {
        0
    };

    [
        EntangledShard {
            nonce,
            session: session ^ 0x0101_0000_0000_0001u64,
            payload_mask: surviving_mask,
            payload_bits: surviving_bit ^ lie_mask,
            tag_mask,
            tag_bits: (base_frame.tag & tag_mask) ^ lie_tag,
        },
        EntangledShard {
            nonce,
            session: session ^ 0x0202_0000_0000_0002u64,
            payload_mask: 0,
            payload_bits: 0,
            tag_mask: 0,
            tag_bits: 0,
        },
        EntangledShard {
            nonce,
            session: session ^ 0x0303_0000_0000_0003u64,
            payload_mask: 0,
            payload_bits: 0,
            tag_mask: 0,
            tag_bits: 0,
        },
    ]
}

fn eternal_origin_reconstruct(shards: &[EntangledShard; 3]) -> Result<u16, &'static str> {
    let payload_coverage = shards
        .iter()
        .fold(0u16, |acc, shard| acc | shard.payload_mask);
    if payload_coverage.count_ones() <= 1 {
        return Err("information_theoretic_limit");
    }

    quantum_reconstruct_entangled(shards)
}

fn run_ghost_protocol_case(
    payload: u16,
    session: u64,
    base_nonce: u32,
    jitter_seed: u64,
    spectral_seed: u64,
    byzantine_seed: u64,
) -> Result<(), &'static str> {
    let kernel = MorphicKernel::new();
    let liars = liar_positions_count::<6>(byzantine_seed);
    let context = ghost_protocol_context();
    let target_preset = kernel.planner().select_preset(&context);
    let mut current_preset = radnet_morphic_kernel::ProfilePreset::Default;
    let mut window = ReplayWindow::new(base_nonce, 8);
    let mut nonce_cursor = base_nonce;

    for hop in 0..12 {
        let jitter_packets =
            1 + (((jitter_seed.rotate_left((hop * 5) as u32) >> (hop % 8)) & 0x0F) as u32 % 10);
        let spectral_hole_open = ((spectral_seed.rotate_left(hop as u32) ^ session) % 20) == 0;
        nonce_cursor = nonce_cursor.saturating_add(jitter_packets);

        if !spectral_hole_open
            || nonce_cursor > window.expected_nonce + window.max_future_skew_packets
        {
            let sync_a = SecureFrame32::encode(0xAAAA, nonce_cursor, session);
            let sync_b = SecureFrame32::encode(0x5555, nonce_cursor + 1, session);
            resync_window(&sync_a, &sync_b, session, &mut window).map_err(packet_error_label)?;
            nonce_cursor = nonce_cursor.saturating_add(2);
        }

        let mut radio = MockRadio::default();
        let writes = kernel
            .apply_preset(&mut radio, current_preset, target_preset)
            .map_err(|_| "apply_failed")?;
        if !radio.standby_called || !radio.committed {
            return Err("invalid_radio_transition");
        }
        if current_preset != target_preset && writes.is_empty() {
            return Err("missing_morphic_switch");
        }
        current_preset = target_preset;

        let honest_frame = SecureFrame32::encode(payload, nonce_cursor, session);

        if liars.contains(&hop) {
            let spoof = SecureFrame32 {
                nonce: nonce_cursor,
                wire: honest_frame.wire ^ (0xAB00 + hop as u32),
                tag: honest_frame.tag ^ (0xBAD0_F00D_u64 + hop as u64),
            };
            if accept_frame(&spoof, session, &mut window).is_ok() {
                return Err("spoof_accepted");
            }
        }

        let decoded =
            accept_frame(&honest_frame, session, &mut window).map_err(packet_error_label)?;
        if decoded != payload {
            return Err("payload_corrupted");
        }
    }

    Ok(())
}

#[test]
fn ten_hop_profile_pipeline_stays_deterministic() {
    let kernel = MorphicKernel::new();
    let hop_contexts = vec![
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
        Context::favorable(),
        Context::constrained(),
    ];

    let mut current = radnet_morphic_kernel::PhyProfile::lora_default();
    let mut transitions = 0usize;
    let mut total_writes = 0usize;

    for context in hop_contexts {
        let target = kernel.planner().generate_phy(&context);
        target.validate().unwrap();

        let mut radio = MockRadio::default();
        let writes = kernel.apply_profile(&mut radio, &current, &target).unwrap();

        assert!(radio.standby_called);
        assert!(radio.committed);
        assert_eq!(radio.writes, writes);

        if current != target {
            assert!(!writes.is_empty());
            transitions += 1;
            total_writes += writes.len();
        }

        current = target;
    }

    current.validate().unwrap();
    assert_eq!(current.bandwidth, Bandwidth::Khz125);
    assert_eq!(current.spreading_factor, Some(SpreadingFactor::Sf11));
    assert_eq!(current.coding_rate, Some(CodingRate::Cr48));
    assert_eq!(current.tx_power_dbm, 17);
    assert_eq!(transitions, 10);
    assert!(total_writes >= 20);
}

#[test]
fn test_entropy_shield_integrity() {
    let session = 0xA4D1_55E7_1020_33CCu64;
    let mut wire_values = HashSet::new();

    for nonce in 0u32..1_000 {
        let payload = 0x0101u16 ^ (nonce as u16).wrapping_mul(17);
        let frame = SecureFrame32::encode(payload, nonce, session);
        let decoded = frame.decode(session).unwrap();

        assert_eq!(decoded, payload);
        assert!(
            wire_values.insert(frame.wire),
            "duplicate wire value for nonce {nonce}"
        );
    }

    assert_eq!(wire_values.len(), 1_000);
}

#[test]
fn test_byzantine_node_detection() {
    let session = 0x55AA_10FE_C011_EC7Du64;
    let frame = SecureFrame32::encode(0x0101, 7, session);
    let poisoned = SecureFrame32 {
        wire: frame.wire ^ 0x0040,
        ..frame
    };

    let err = poisoned.decode(session).unwrap_err();
    assert_eq!(err, PacketError::AuthTagMismatch);

    let recovered = SecureFrame32::encode(0x0101, 8, session);
    let decoded = recovered.decode(session).unwrap();
    assert_eq!(decoded, 0x0101);
}

#[test]
fn test_adaptive_power_manager_rebalances_tx_power_without_breaking_morphic_lock() {
    let kernel = MorphicKernel::new();
    let clean_context = Context {
        noise_floor_dbm: -129,
        snr_db: 15,
        battery_mv: 3720,
        latency_budget_ms: 120,
        link_margin_db: 22,
    };
    let degraded_context = Context {
        noise_floor_dbm: -118,
        snr_db: 1,
        battery_mv: 3720,
        latency_budget_ms: 120,
        link_margin_db: 8,
    };

    let current = kernel.planner().generate_phy(&clean_context);
    let target = kernel.planner().generate_phy(&degraded_context);

    assert_eq!(current.bandwidth, Bandwidth::Khz125);
    assert_eq!(target.bandwidth, Bandwidth::Khz125);
    assert_eq!(current.spreading_factor, Some(SpreadingFactor::Sf9));
    assert_eq!(target.spreading_factor, Some(SpreadingFactor::Sf9));
    assert_eq!(current.coding_rate, Some(CodingRate::Cr45));
    assert_eq!(target.coding_rate, Some(CodingRate::Cr45));
    assert!(target.tx_power_dbm > current.tx_power_dbm);

    let mut radio = MockRadio::default();
    let writes = kernel.apply_profile(&mut radio, &current, &target).unwrap();
    assert!(radio.standby_called);
    assert!(radio.committed);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].register, "REG_PA_CONFIG");
    assert_eq!(writes[0].value, target.tx_power_dbm as u32);

    let session = 0x44AA_7711_9900_BEEFu64;
    let mut window = ReplayWindow::new(4_000, 8);
    let clean_frame = SecureFrame32::encode(0x0101, 4_000, session);
    let degraded_frame = SecureFrame32::encode(0x0101, 4_001, session);

    assert_eq!(
        accept_frame(&clean_frame, session, &mut window).unwrap(),
        0x0101
    );
    assert_eq!(
        accept_frame(&degraded_frame, session, &mut window).unwrap(),
        0x0101
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        max_shrink_iters: 0,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_singularity_resilience(
        payload_mask in any::<u16>(),
        interference_db in 12u8..=16u8,
        doppler_shift_khz in -100i16..=100i16,
        clock_skew_secs in 5u8..=10u8,
        base_nonce in 10_000u32..20_000u32,
        byzantine_seed in any::<u64>(),
        session in any::<u64>(),
    ) {
        static DID_TIMING_CHECK: AtomicBool = AtomicBool::new(false);

        let payload = 0x0101 ^ payload_mask;
        let session = session ^ 0xA4D1_55E7_1020_33CCu64;

        prop_assert!(
            run_singularity_case(
                payload,
                session,
                base_nonce,
                clock_skew_secs,
                interference_db,
                doppler_shift_khz,
                byzantine_seed,
            )
            .is_ok()
        );

        if !cfg!(debug_assertions) && !DID_TIMING_CHECK.swap(true, Ordering::Relaxed) {
            let start = Instant::now();
            for offset in 0..512u32 {
                run_singularity_case(
                    0x0101,
                    session,
                    base_nonce + offset,
                    clock_skew_secs,
                    interference_db,
                    doppler_shift_khz,
                    byzantine_seed ^ u64::from(offset),
                )
                .unwrap();
            }
            let average_packet_time_us = start.elapsed().as_secs_f64() * 1_000_000.0 / 512.0;
            prop_assert!(
                average_packet_time_us < 5.0,
                "average packet execution time exceeded budget: {average_packet_time_us:.3} us"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4096,
        max_shrink_iters: 0,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_quantum_entanglement_recovery(
        input_cmd in 0u16..=u16::MAX,
        base_nonce in 170_000u32..200_000u32,
        poison_seed in any::<u64>(),
        session in any::<u64>(),
    ) {
        static DID_ENTANGLEMENT_TIMING_CHECK: AtomicBool = AtomicBool::new(false);

        let session = session ^ 0x7171_5151_C0DE_1ACEu64;
        prop_assert!(
            run_quantum_entanglement_case(
                input_cmd,
                base_nonce,
                session,
                poison_seed,
            )
            .is_ok()
        );

        if !cfg!(debug_assertions) && !DID_ENTANGLEMENT_TIMING_CHECK.swap(true, Ordering::Relaxed) {
            let nonce = base_nonce.saturating_add(12);
            let route_sessions = [
                session ^ 0xD1D1_0000_0000_0001u64,
                session ^ 0xE2E2_0000_0000_0002u64,
                session ^ 0xF3F3_0000_0000_0003u64,
            ];
            let shards = [
                build_entangled_shard(0x0101, nonce, route_sessions[0], 0),
                build_entangled_shard(0x0101, nonce, route_sessions[1], 1),
                build_entangled_shard(0x0101, nonce, route_sessions[2], 2),
            ];

            let start = Instant::now();
            for _ in 0..4096 {
                let recovered = quantum_reconstruct_entangled(&shards).unwrap();
                prop_assert_eq!(recovered, 0x0101);
            }
            let average_reconstruction_time_ns =
                start.elapsed().as_secs_f64() * 1_000_000_000.0 / 4096.0;
            prop_assert!(
                average_reconstruction_time_ns < 150.0,
                "average entanglement reconstruction time exceeded budget: {average_reconstruction_time_ns:.3} ns"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4096,
        max_shrink_iters: 0,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_schrodinger_packet_collapse(
        input_cmd in 0u16..=u16::MAX,
        base_nonce in 130_000u32..160_000u32,
        route_seed in any::<u64>(),
        observer_seed in any::<u64>(),
        session in any::<u64>(),
    ) {
        static DID_COLLAPSE_TIMING_CHECK: AtomicBool = AtomicBool::new(false);

        let session = session ^ 0x5C11_0D1E_FEED_FACEu64;
        prop_assert!(
            run_schrodinger_collapse_case(
                input_cmd,
                base_nonce,
                session,
                route_seed,
                observer_seed,
            )
            .is_ok()
        );

        if !cfg!(debug_assertions) && !DID_COLLAPSE_TIMING_CHECK.swap(true, Ordering::Relaxed) {
            let route_sessions = [
                session ^ 0xA1A1_1111_0000_0001u64,
                session ^ 0xB2B2_2222_0000_0002u64,
                session ^ 0xC3C3_3333_0000_0003u64,
            ];
            let valid_candidates = [
                (SecureFrame32::encode(0x0101, base_nonce, route_sessions[0]), route_sessions[0]),
                (SecureFrame32::encode(0x0101, base_nonce, route_sessions[1]), route_sessions[1]),
                (SecureFrame32::encode(0x0101, base_nonce, route_sessions[2]), route_sessions[2]),
            ];

            let start = Instant::now();
            for _ in 0..4096 {
                let recovered = quantum_collapse_decode(&valid_candidates).unwrap();
                prop_assert_eq!(recovered, 0x0101);
            }
            let average_collapse_time_ns =
                start.elapsed().as_secs_f64() * 1_000_000_000.0 / 4096.0;
            prop_assert!(
                average_collapse_time_ns < 100.0,
                "average collapse time exceeded budget: {average_collapse_time_ns:.3} ns"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4096,
        max_shrink_iters: 0,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_quantum_ghost_resilience(
        input_cmd in 0u16..=u16::MAX,
        base_nonce in 90_000u32..120_000u32,
        bitflip_seed in any::<u64>(),
        observer_seed in any::<u64>(),
        byzantine_seed in any::<u64>(),
        session in any::<u64>(),
    ) {
        static DID_QUANTUM_TIMING_CHECK: AtomicBool = AtomicBool::new(false);

        let session = session ^ 0x51A9_C0DE_FEED_BEEFu64;
        let env = QuantumChaosEnv::new()
            .bit_flip_storm(0.10)
            .byzantine_hell(0.70)
            .observer_collapse(true)
            .stateless_hops(true)
            .time_travel_skew_packets(20);

        prop_assert!(
            run_quantum_ghost_case(
                input_cmd,
                session,
                base_nonce,
                env,
                bitflip_seed,
                observer_seed,
                byzantine_seed,
            )
            .is_ok()
        );

        if !cfg!(debug_assertions) && !DID_QUANTUM_TIMING_CHECK.swap(true, Ordering::Relaxed) {
            let mut total_window = ReplayWindow::new(base_nonce, 8);
            let start = Instant::now();
            for offset in 0..2048u32 {
                let frame = SecureFrame32::encode(
                    0x0101,
                    base_nonce + offset,
                    session ^ u64::from(offset),
                );
                let decoded = accept_frame(&frame, session ^ u64::from(offset), &mut total_window)
                    .unwrap();
                prop_assert_eq!(decoded, 0x0101);
            }

            let average_decode_time_ns =
                start.elapsed().as_secs_f64() * 1_000_000_000.0 / 2048.0;
            prop_assert!(
                average_decode_time_ns < 200.0,
                "average decode/accept time exceeded budget: {average_decode_time_ns:.3} ns"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2048,
        max_shrink_iters: 0,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_ghost_protocol_singularity(
        input_cmd in 0u16..=u16::MAX,
        base_nonce in 50_000u32..80_000u32,
        jitter_seed in any::<u64>(),
        spectral_seed in any::<u64>(),
        byzantine_seed in any::<u64>(),
        session in any::<u64>(),
    ) {
        let session = session ^ 0xDEAD_C0DE_CAFE_BABEu64;

        prop_assert!(
            run_ghost_protocol_case(
                input_cmd,
                session,
                base_nonce,
                jitter_seed,
                spectral_seed,
                byzantine_seed,
            )
            .is_ok()
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 4096,
        max_shrink_iters: 0,
        .. ProptestConfig::default()
    })]

    #[test]
    fn test_eternal_origin_protocol_fail_closed(
        input_cmd in 0u16..=u16::MAX,
        base_nonce in 210_000u32..240_000u32,
        poison_seed in any::<u64>(),
        session in any::<u64>(),
    ) {
        static DID_ETERNAL_TIMING_CHECK: AtomicBool = AtomicBool::new(false);

        let session = session ^ 0xE7E7_0A11_F1A1_CED0u64;
        let shards = build_eternal_origin_shards(
            input_cmd,
            base_nonce.saturating_add(12),
            session,
            poison_seed,
        );

        prop_assert_eq!(
            eternal_origin_reconstruct(&shards),
            Err("information_theoretic_limit")
        );

        if !cfg!(debug_assertions) && !DID_ETERNAL_TIMING_CHECK.swap(true, Ordering::Relaxed) {
            let reference = build_eternal_origin_shards(
                0x0101,
                base_nonce.saturating_add(12),
                session,
                poison_seed,
            );

            let start = Instant::now();
            for _ in 0..4096 {
                let result = eternal_origin_reconstruct(&reference);
                prop_assert_eq!(result, Err("information_theoretic_limit"));
            }
            let average_fail_closed_time_ns =
                start.elapsed().as_secs_f64() * 1_000_000_000.0 / 4096.0;
            prop_assert!(
                average_fail_closed_time_ns < 50.0,
                "average fail-closed detection time exceeded budget: {average_fail_closed_time_ns:.3} ns"
            );
        }
    }
}
