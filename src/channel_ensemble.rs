//! # Channel Ensemble — Multi-Band PLKA Контур
//!
//! Дополнительный контур поверх существующего MorphicKernel.
//! Не модифицирует phy.rs / kernel.rs — встраивается как отдельный слой.
//!
//! ## Архитектура
//!
//! ```text
//! [LoRa 863-870 MHz, 125ch] ──┐
//! [BLE  2402-2480 MHz, 40ch] ──┤  BruFusion ──> Multiband ключ
//! [WiFi 2412-2472 MHz, 13ch] ──┘       ↑
//!                                  Theorem IV BRU (~~)
//! ```
//!
//! ## Патентная новизна
//!
//! - LoRa full-band: 125 каналов × 56 kHz шаг = 7 MHz покрытие
//! - Cross-technology: LoRa + BLE + WiFi независимые физические процессы
//! - BRU fusion: `u_out = min(u1, u2, u3)` — неопределённость не растёт
//! - Атакующий не может перехватить все 3 технологии одновременно с одной точки

/// Тип физического канала
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChannelTechnology {
    /// LoRa SX1262 (EU ISM 863-870 MHz)
    LoRa,
    /// Bluetooth Low Energy (2402-2480 MHz, 40 каналов)
    Ble,
    /// WiFi 2.4 GHz (2412-2472 MHz, 13 каналов)
    Wifi24,
}

/// Один физический канал в ансамбле
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnsembleChannel {
    pub technology: ChannelTechnology,
    pub frequency_hz: u32,
    pub channel_index: u8,
}

/// Измерение канала (получено с железа)
#[derive(Debug, Clone, Copy)]
pub struct ChannelMeasurement {
    pub channel: EnsembleChannel,
    /// RSSI в dBm × 2 (целочисленное, как на SX1262)
    pub rssi_half_db: i16,
    /// SNR в dB × 4
    pub snr_quarter_db: i16,
    /// Turnaround в мкс (reciprocity marker)
    pub turnaround_us: u32,
    /// Бит энтропии извлечённый из этого измерения (0 или 1)
    pub entropy_bit: Option<u8>,
}

/// Результат BRU-слияния нескольких каналов (Theorem IV)
#[derive(Debug, Clone, Copy)]
pub struct BruFusionResult {
    /// Суммарные биты из всех каналов
    pub total_bits: u16,
    /// Неопределённость (BRU: min из всех каналов, не растёт)
    pub uncertainty: f32,
    /// Совместный хэш (SHA256 первые 4 байта для проверки)
    pub key_fingerprint: u32,
    /// Число технологий задействовано
    pub technology_count: u8,
}

/// Full-band hop план для LoRa (EU ISM 863-870 MHz)
///
/// 125 каналов с шагом 56 kHz:
/// ch0=863_000_000, ch1=863_056_000, ..., ch124=869_944_000
#[derive(Debug, Clone)]
pub struct FullBandHopPlan {
    channels: [u32; 125],
    current_index: usize,
    /// Псевдослучайная последовательность прыжков (детерминированная)
    hop_sequence: [u8; 125],
}

impl FullBandHopPlan {
    const CHANNEL_STEP_HZ: u32 = 56_000;
    const BASE_FREQ_HZ: u32 = 863_000_000;
    const NUM_CHANNELS: usize = 125;

    /// Создать план с детерминированной hop-последовательностью
    /// seed — физическая энтропия (из предыдущей keygen сессии)
    pub fn new(seed: u32) -> Self {
        let mut channels = [0u32; 125];
        for i in 0..Self::NUM_CHANNELS {
            channels[i] = Self::BASE_FREQ_HZ + (i as u32) * Self::CHANNEL_STEP_HZ;
        }

        // Детерминированная перестановка Fisher-Yates на seed
        let mut hop_sequence: [u8; 125] = core::array::from_fn(|i| i as u8);
        let mut rng = seed;
        for i in (1..Self::NUM_CHANNELS).rev() {
            rng = rng
                .wrapping_mul(6364136223846793005_u64 as u32)
                .wrapping_add(1442695040888963407_u64 as u32);
            let j = (rng as usize) % (i + 1);
            hop_sequence.swap(i, j);
        }

        Self {
            channels,
            current_index: 0,
            hop_sequence,
        }
    }

    /// Следующий канал в hop-последовательности
    pub fn next_channel(&mut self) -> EnsembleChannel {
        let seq_idx = self.hop_sequence[self.current_index % Self::NUM_CHANNELS] as usize;
        self.current_index = (self.current_index + 1) % Self::NUM_CHANNELS;
        EnsembleChannel {
            technology: ChannelTechnology::LoRa,
            frequency_hz: self.channels[seq_idx],
            channel_index: seq_idx as u8,
        }
    }

    /// Текущая позиция в последовательности
    pub fn position(&self) -> usize {
        self.current_index
    }

    /// Сколько уникальных каналов доступно
    pub fn channel_count(&self) -> usize {
        Self::NUM_CHANNELS
    }

    /// Частота по индексу (без сдвига последовательности)
    pub fn freq_at(&self, index: usize) -> u32 {
        self.channels[index % Self::NUM_CHANNELS]
    }
}

/// BLE-каналы для cross-technology слоя
/// 40 каналов: 2402, 2404, ..., 2480 MHz (шаг 2 MHz)
#[derive(Debug, Clone)]
pub struct BleCrossLayer {
    channels: [u32; 40],
    current_index: usize,
}

impl BleCrossLayer {
    const BASE_FREQ_HZ: u32 = 2_402_000_000;
    const CHANNEL_STEP_HZ: u32 = 2_000_000;
    const NUM_CHANNELS: usize = 40;

    pub fn new() -> Self {
        let mut channels = [0u32; 40];
        for i in 0..Self::NUM_CHANNELS {
            channels[i] = Self::BASE_FREQ_HZ + (i as u32) * Self::CHANNEL_STEP_HZ;
        }
        Self {
            channels,
            current_index: 0,
        }
    }

    pub fn next_channel(&mut self) -> EnsembleChannel {
        let idx = self.current_index % Self::NUM_CHANNELS;
        self.current_index += 1;
        EnsembleChannel {
            technology: ChannelTechnology::Ble,
            frequency_hz: self.channels[idx],
            channel_index: idx as u8,
        }
    }

    pub fn channel_count(&self) -> usize {
        Self::NUM_CHANNELS
    }
}

impl Default for BleCrossLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// WiFi 2.4 GHz cross-layer
/// 13 каналов: 2412, 2417, ..., 2472 MHz (шаг 5 MHz)
#[derive(Debug, Clone)]
pub struct WifiCrossLayer {
    channels: [u32; 13],
    current_index: usize,
}

impl WifiCrossLayer {
    const BASE_FREQ_HZ: u32 = 2_412_000_000;
    const CHANNEL_STEP_HZ: u32 = 5_000_000;
    const NUM_CHANNELS: usize = 13;

    pub fn new() -> Self {
        let mut channels = [0u32; 13];
        for i in 0..Self::NUM_CHANNELS {
            channels[i] = Self::BASE_FREQ_HZ + (i as u32) * Self::CHANNEL_STEP_HZ;
        }
        Self {
            channels,
            current_index: 0,
        }
    }

    pub fn next_channel(&mut self) -> EnsembleChannel {
        let idx = self.current_index % Self::NUM_CHANNELS;
        self.current_index += 1;
        EnsembleChannel {
            technology: ChannelTechnology::Wifi24,
            frequency_hz: self.channels[idx],
            channel_index: idx as u8,
        }
    }

    pub fn channel_count(&self) -> usize {
        Self::NUM_CHANNELS
    }
}

impl Default for WifiCrossLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// BRU-слияние измерений (Theorem IV: u_out = min(u1, u2, ...) — не накапливается)
///
/// Принимает измерения из N каналов разных технологий,
/// возвращает объединённый entropy score и fingerprint.
pub struct BruFusion;

impl BruFusion {
    /// Слить измерения из нескольких каналов в единый ключевой материал
    ///
    /// Реализует BRU-оператор `~~` для multi-source:
    /// - entropy объединяется XOR + hash цепочкой
    /// - uncertainty = min(all_uncertainties) — не растёт!
    pub fn fuse(measurements: &[ChannelMeasurement]) -> BruFusionResult {
        if measurements.is_empty() {
            return BruFusionResult {
                total_bits: 0,
                uncertainty: 1.0,
                key_fingerprint: 0,
                technology_count: 0,
            };
        }

        let mut bits: u16 = 0;
        let mut uncertainty_min: f32 = 1.0;
        let mut hash_acc: u32 = 0x811c_9dc5; // FNV-1a basis

        let mut tech_seen = [false; 3];

        for m in measurements {
            if let Some(bit) = m.entropy_bit {
                bits += 1;

                // FNV-1a hash цепочка (deterministic, no_std safe)
                let byte = bit ^ ((m.rssi_half_db & 0xFF) as u8) ^ ((m.turnaround_us & 0xFF) as u8);
                hash_acc ^= byte as u32;
                hash_acc = hash_acc.wrapping_mul(0x0100_0193);
            }

            // BRU uncertainty: нормализованный RSSI как proxy
            // Чем сильнее сигнал (ближе к 0 dBm) — тем меньше неопределённость
            let rssi_norm = (m.rssi_half_db.abs() as f32) / 256.0; // нормализация
            let u_channel = (rssi_norm).clamp(0.0, 1.0);

            // BRU правило: u_out = min(u_in) — Theorem IV
            if u_channel < uncertainty_min {
                uncertainty_min = u_channel;
            }

            let tech_idx = m.channel.technology as usize;
            if tech_idx < 3 {
                tech_seen[tech_idx] = true;
            }
        }

        let technology_count = tech_seen.iter().filter(|&&v| v).count() as u8;

        BruFusionResult {
            total_bits: bits,
            uncertainty: uncertainty_min,
            key_fingerprint: hash_acc,
            technology_count,
        }
    }

    /// Оценка безопасности ансамбля
    ///
    /// Возвращает score 0.0–1.0:
    /// - 1.0 = все 3 технологии + >128 бит + low uncertainty
    /// - <0.5 = недостаточно для production ключа
    pub fn security_score(result: &BruFusionResult) -> f32 {
        let bit_score = (result.total_bits as f32 / 128.0).clamp(0.0, 1.0);
        let tech_score = (result.technology_count as f32) / 3.0;
        let uncertainty_score = 1.0 - result.uncertainty;

        // Взвешенная сумма (BRU: uncertainty не накапливается)
        0.4 * bit_score + 0.3 * tech_score + 0.3 * uncertainty_score
    }
}

/// Полный Multi-Band PLKA ансамбль
///
/// Управляет всеми тремя контурами и выдаёт сводный ключевой материал.
pub struct ChannelEnsemble {
    pub lora_hop: FullBandHopPlan,
    pub ble_layer: BleCrossLayer,
    pub wifi_layer: WifiCrossLayer,
}

impl ChannelEnsemble {
    /// Создать ансамбль с физической энтропией из предыдущей keygen сессии
    pub fn new(physical_seed: u32) -> Self {
        Self {
            lora_hop: FullBandHopPlan::new(physical_seed),
            ble_layer: BleCrossLayer::new(),
            wifi_layer: WifiCrossLayer::new(),
        }
    }

    /// Суммарное число каналов во всех технологиях
    pub fn total_channel_count(&self) -> usize {
        self.lora_hop.channel_count()
            + self.ble_layer.channel_count()
            + self.wifi_layer.channel_count()
    }

    /// Следующий тройной шаг: один канал от каждой технологии
    pub fn next_triple(&mut self) -> (EnsembleChannel, EnsembleChannel, EnsembleChannel) {
        (
            self.lora_hop.next_channel(),
            self.ble_layer.next_channel(),
            self.wifi_layer.next_channel(),
        )
    }

    /// Слить набор измерений в итоговый результат
    pub fn fuse(&self, measurements: &[ChannelMeasurement]) -> BruFusionResult {
        BruFusion::fuse(measurements)
    }

    /// Оценить безопасность текущей сессии
    pub fn security_score(&self, result: &BruFusionResult) -> f32 {
        BruFusion::security_score(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_band_hop_covers_all_channels() {
        let mut plan = FullBandHopPlan::new(1802543074);
        let mut seen = [false; 125];
        for _ in 0..125 {
            let ch = plan.next_channel();
            assert_eq!(ch.technology, ChannelTechnology::LoRa);
            assert!(ch.frequency_hz >= 863_000_000);
            assert!(ch.frequency_hz <= 869_944_000);
            seen[ch.channel_index as usize] = true;
        }
        // Все 125 каналов должны быть охвачены за один цикл
        assert!(seen.iter().all(|&v| v), "Not all 125 channels visited");
    }

    #[test]
    fn full_band_hop_is_deterministic() {
        let mut p1 = FullBandHopPlan::new(42);
        let mut p2 = FullBandHopPlan::new(42);
        for _ in 0..125 {
            assert_eq!(
                p1.next_channel().frequency_hz,
                p2.next_channel().frequency_hz
            );
        }
    }

    #[test]
    fn full_band_hop_seed_changes_sequence() {
        let mut p1 = FullBandHopPlan::new(42);
        let mut p2 = FullBandHopPlan::new(99);
        let freqs1: [u32; 10] = core::array::from_fn(|_| p1.next_channel().frequency_hz);
        let freqs2: [u32; 10] = core::array::from_fn(|_| p2.next_channel().frequency_hz);
        assert_ne!(
            freqs1, freqs2,
            "Different seeds must give different sequences"
        );
    }

    #[test]
    fn ble_layer_40_channels() {
        let mut ble = BleCrossLayer::new();
        let ch0 = ble.next_channel();
        assert_eq!(ch0.frequency_hz, 2_402_000_000);
        assert_eq!(ch0.technology, ChannelTechnology::Ble);
        let ch1 = ble.next_channel();
        assert_eq!(ch1.frequency_hz, 2_404_000_000);
    }

    #[test]
    fn wifi_layer_13_channels() {
        let mut wifi = WifiCrossLayer::new();
        let ch0 = wifi.next_channel();
        assert_eq!(ch0.frequency_hz, 2_412_000_000);
        assert_eq!(ch0.technology, ChannelTechnology::Wifi24);
    }

    #[test]
    fn bru_fusion_uncertainty_does_not_grow() {
        // Theorem IV: u_out = min(u1, u2) — никогда не растёт
        let measurements = vec![
            ChannelMeasurement {
                channel: EnsembleChannel {
                    technology: ChannelTechnology::LoRa,
                    frequency_hz: 868_000_000,
                    channel_index: 0,
                },
                rssi_half_db: -228, // -114 dBm
                snr_quarter_db: -12,
                turnaround_us: 1_248_000,
                entropy_bit: Some(1),
            },
            ChannelMeasurement {
                channel: EnsembleChannel {
                    technology: ChannelTechnology::Ble,
                    frequency_hz: 2_402_000_000,
                    channel_index: 0,
                },
                rssi_half_db: -140, // -70 dBm (сильнее)
                snr_quarter_db: 20,
                turnaround_us: 500,
                entropy_bit: Some(0),
            },
        ];

        let result = BruFusion::fuse(&measurements);
        // u_out = min(u_lora, u_ble) — должно быть <= каждого из них
        let u_lora = (228.0_f32 / 256.0).clamp(0.0, 1.0);
        let u_ble = (140.0_f32 / 256.0).clamp(0.0, 1.0);
        assert!(result.uncertainty <= u_lora + 1e-5);
        assert!(result.uncertainty <= u_ble + 1e-5);
        assert_eq!(result.total_bits, 2);
    }

    #[test]
    fn bru_fusion_multi_technology_score() {
        let measurements: Vec<ChannelMeasurement> = (0..3)
            .map(|i| ChannelMeasurement {
                channel: EnsembleChannel {
                    technology: match i {
                        0 => ChannelTechnology::LoRa,
                        1 => ChannelTechnology::Ble,
                        _ => ChannelTechnology::Wifi24,
                    },
                    frequency_hz: 868_000_000 + i * 1_000_000,
                    channel_index: i as u8,
                },
                rssi_half_db: -160,
                snr_quarter_db: 8,
                turnaround_us: 1_000_000,
                entropy_bit: Some((i % 2) as u8),
            })
            .collect();

        let result = BruFusion::fuse(&measurements);
        assert_eq!(result.technology_count, 3);
        let score = BruFusion::security_score(&result);
        // С 3 технологиями score должен быть > 0.3
        assert!(score > 0.3, "score={score}");
    }

    #[test]
    fn ensemble_total_channels() {
        let ens = ChannelEnsemble::new(1802543074);
        // 125 LoRa + 40 BLE + 13 WiFi = 178
        assert_eq!(ens.total_channel_count(), 178);
    }

    #[test]
    fn ensemble_next_triple_gives_all_technologies() {
        let mut ens = ChannelEnsemble::new(1802543074);
        let (lora, ble, wifi) = ens.next_triple();
        assert_eq!(lora.technology, ChannelTechnology::LoRa);
        assert_eq!(ble.technology, ChannelTechnology::Ble);
        assert_eq!(wifi.technology, ChannelTechnology::Wifi24);
    }
}
