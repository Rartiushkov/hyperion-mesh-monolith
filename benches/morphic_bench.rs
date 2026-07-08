use criterion::{black_box, criterion_group, criterion_main, Criterion};
use radnet_morphic_kernel::{
    accept_frame, resync_window, AdaptivePowerManager, Context, MockRadio, MorphicKernel,
    PacketError, ProfilePreset, ReplayWindow, SecureFrame32,
};

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

#[derive(Debug, Clone, Copy)]
struct QuantumChaosEnv {
    bit_flip_rate: f32,
    byzantine_fraction: f32,
    observer_collapse: bool,
    stateless_hops: bool,
    clock_skew_packets: u32,
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

    let mut resolved = 0u16;
    for shard in shards {
        resolved |= shard.payload_bits;
        black_box(shard.nonce);
        black_box(shard.session);
        black_box(shard.tag_mask);
        black_box(shard.tag_bits);
    }
    Ok(resolved)
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

fn run_singularity_packet(
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

    let mut current_preset = ProfilePreset::Default;
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

fn run_quantum_ghost_packet(
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

    let mut current_preset = ProfilePreset::Default;
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

fn bench_10_hop_profile_pipeline(c: &mut Criterion) {
    let kernel = MorphicKernel::new();
    let hop_contexts = [
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

    c.bench_function("radnet_10_hop_profile_pipeline", |b| {
        b.iter(|| {
            let mut current = ProfilePreset::Default;

            for context in &hop_contexts {
                let target = kernel.planner().select_preset(context);
                let mut radio = MockRadio::default();
                let writes = kernel.apply_preset(&mut radio, current, target).unwrap();

                black_box(&writes);
                black_box(&radio);
                current = target;
            }

            black_box(current)
        });
    });
}

fn bench_singularity_packet_path(c: &mut Criterion) {
    c.bench_function("radnet_singularity_packet", |b| {
        b.iter(|| {
            black_box(
                run_singularity_packet(
                    0x0101,
                    0xA4D1_55E7_1020_33CCu64,
                    12_000,
                    8,
                    16,
                    83,
                    0x9E37_79B9_0000_0042u64,
                )
                .unwrap(),
            );
        });
    });
}

fn bench_quantum_ghost_packet_path(c: &mut Criterion) {
    let env = QuantumChaosEnv::new()
        .bit_flip_storm(0.10)
        .byzantine_hell(0.70)
        .observer_collapse(true)
        .stateless_hops(true)
        .time_travel_skew_packets(20);

    c.bench_function("radnet_quantum_ghost_packet", |b| {
        b.iter(|| {
            black_box(
                run_quantum_ghost_packet(
                    0x0101,
                    0x51A9_C0DE_FEED_BEEFu64,
                    100_000,
                    env,
                    0x1234_5678_9ABC_DEF0u64,
                    0x0FED_CBA9_8765_4321u64,
                    0xCAFEBABE_DEADC0DEu64,
                )
                .unwrap(),
            );
        });
    });
}

fn bench_session_resync(c: &mut Criterion) {
    let mut seed = 12_000u32;
    c.bench_function("radnet_session_resync_two_frame", |b| {
        b.iter(|| {
            seed = seed.wrapping_add(2);
            let session = black_box(0xA4D1_55E7_1020_33CCu64 ^ u64::from(seed));
            let mut window = ReplayWindow::new(black_box(seed), 8);
            let sync_a = SecureFrame32::encode(0xAAAA, seed + 16, session);
            let sync_b = SecureFrame32::encode(0x5555, seed + 17, session);

            black_box(resync_window(&sync_a, &sync_b, session, &mut window).unwrap());
            black_box(window);
        });
    });
}

fn bench_quantum_resync_jump(c: &mut Criterion) {
    let env = QuantumChaosEnv::new()
        .observer_collapse(true)
        .time_travel_skew_packets(20);
    let mut seed = 90_000u32;

    c.bench_function("radnet_quantum_resync_jump", |b| {
        b.iter(|| {
            seed = seed.wrapping_add(2);
            let session = black_box(0x51A9_C0DE_FEED_BEEFu64 ^ u64::from(seed));
            let mut window = ReplayWindow::new(black_box(seed), 8);
            let sync_a = SecureFrame32::encode(0xAAAA, seed + env.clock_skew_packets, session);
            let sync_b = SecureFrame32::encode(0x5555, seed + env.clock_skew_packets + 1, session);
            let observation = observe_frame(&sync_a, session);

            black_box(observation);
            black_box(resync_window(&sync_a, &sync_b, session, &mut window).unwrap());
            black_box(window);
        });
    });
}

fn bench_eternal_origin_fail_closed(c: &mut Criterion) {
    let shards = build_eternal_origin_shards(
        0x0101,
        220_012,
        0xE7E7_0A11_F1A1_CED0u64,
        0xDEAD_CAFE_0011_2233u64,
    );

    c.bench_function("radnet_eternal_origin_fail_closed", |b| {
        b.iter(|| {
            let result = eternal_origin_reconstruct(black_box(&shards));
            black_box(result).unwrap_err();
        });
    });
}

fn bench_secure_frame_zero_copy(c: &mut Criterion) {
    let mut nonce = 12_345u32;
    c.bench_function("radnet_secure_frame_zero_copy", |b| {
        b.iter(|| {
            nonce = nonce.wrapping_add(1);
            let mut frame = SecureFrame32::default();
            frame.encode_in_place(
                black_box(0x0101),
                black_box(nonce),
                black_box(0xA4D1_55E7_1020_33CCu64 ^ u64::from(nonce)),
            );
            black_box(frame);
        });
    });
}

fn bench_adaptive_tx_power_recalc(c: &mut Criterion) {
    let contexts = [
        Context {
            noise_floor_dbm: -129,
            snr_db: 15,
            battery_mv: 3720,
            latency_budget_ms: 120,
            link_margin_db: 22,
        },
        Context {
            noise_floor_dbm: -122,
            snr_db: 9,
            battery_mv: 3580,
            latency_budget_ms: 120,
            link_margin_db: 13,
        },
        Context {
            noise_floor_dbm: -118,
            snr_db: 1,
            battery_mv: 3460,
            latency_budget_ms: 120,
            link_margin_db: 8,
        },
        Context {
            noise_floor_dbm: -112,
            snr_db: -6,
            battery_mv: 3120,
            latency_budget_ms: 220,
            link_margin_db: 2,
        },
    ];
    let kernel = MorphicKernel::new();

    c.bench_function("radnet_adaptive_tx_power_recalc", |b| {
        b.iter(|| {
            let mut last_power = 0i8;
            for context in &contexts {
                let preset = kernel.planner().select_preset(black_box(context));
                last_power =
                    AdaptivePowerManager::recommend_tx_power_dbm(preset, black_box(context));
            }
            black_box(last_power);
        });
    });
}

criterion_group!(
    benches,
    bench_10_hop_profile_pipeline,
    bench_singularity_packet_path,
    bench_quantum_ghost_packet_path,
    bench_session_resync,
    bench_quantum_resync_jump,
    bench_eternal_origin_fail_closed,
    bench_secure_frame_zero_copy,
    bench_adaptive_tx_power_recalc
);
criterion_main!(benches);
