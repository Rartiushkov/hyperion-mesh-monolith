//! Lock-free RAMAN ledger for JIT-DEX swaps.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapError {
    InvalidAccount,
    InsufficientBalance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegotechAvatar {
    pub avatar_id: Uuid,
    pub api_key: String,
    pub is_active: bool,
}

/// Minimal lock-free multi-currency ledger.
///
/// Accounts are indexed by a compact `u16` id.  USDT and EURC balances are
/// stored in separate `AtomicU64` arrays so that a cross-currency swap can be
/// executed without a global mutex — only per-account CAS loops.
pub struct RamanLedger {
    usdt: Vec<AtomicU64>,
    eurc: Vec<AtomicU64>,
    avatars: Arc<RwLock<Vec<CodegotechAvatar>>>,
    avatar_cursor: AtomicUsize,
}

impl RamanLedger {
    /// Create a ledger with `accounts` slots, each pre-funded with the given
    /// initial balances (in micro-units).
    pub fn new(accounts: usize, initial_usdt_micro: u64, initial_eurc_micro: u64) -> Self {
        Self {
            usdt: (0..accounts)
                .map(|_| AtomicU64::new(initial_usdt_micro))
                .collect(),
            eurc: (0..accounts)
                .map(|_| AtomicU64::new(initial_eurc_micro))
                .collect(),
            avatars: Arc::new(RwLock::new(Vec::new())),
            avatar_cursor: AtomicUsize::new(0),
        }
    }

    pub fn avatars(&self) -> Arc<RwLock<Vec<CodegotechAvatar>>> {
        self.avatars.clone()
    }

    pub fn replace_avatars(&self, avatars: Vec<CodegotechAvatar>) {
        if let Ok(mut guard) = self.avatars.write() {
            *guard = avatars;
            self.avatar_cursor.store(0, Ordering::Release);
        }
    }

    /// Round-robin rate-limit optimizer for Codegotech API calls.
    pub fn route_via_optimal_avatar(&self) -> Option<CodegotechAvatar> {
        let guard = self.avatars.read().ok()?;
        if guard.is_empty() {
            return None;
        }
        let active: Vec<&CodegotechAvatar> =
            guard.iter().filter(|avatar| avatar.is_active).collect();
        if active.is_empty() {
            return None;
        }
        let idx = self.avatar_cursor.fetch_add(1, Ordering::AcqRel) % active.len();
        Some(active[idx].clone())
    }

    /// Read a USDT balance.
    pub fn usdt_balance(&self, account: u16) -> Option<u64> {
        self.usdt
            .get(account as usize)
            .map(|a| a.load(Ordering::Relaxed))
    }

    /// Read an EURC balance.
    pub fn eurc_balance(&self, account: u16) -> Option<u64> {
        self.eurc
            .get(account as usize)
            .map(|a| a.load(Ordering::Relaxed))
    }

    /// Atomically debit `amount_usdt_micro` USDT from `src` and credit the
    /// equivalent EURC amount to `dst` using `fx_rate` (EURC per 1M USDT).
    ///
    /// Returns the credited EURC micro-amount on success.
    pub fn execute_jit_dex_swap(
        &self,
        src: u16,
        dst: u16,
        amount_usdt_micro: u64,
        fx_rate: u64,
    ) -> Result<u64, SwapError> {
        let src_usdt = self
            .usdt
            .get(src as usize)
            .ok_or(SwapError::InvalidAccount)?;
        let dst_eurc = self
            .eurc
            .get(dst as usize)
            .ok_or(SwapError::InvalidAccount)?;

        let eurc_amount = amount_usdt_micro.saturating_mul(fx_rate) / 1_000_000;

        loop {
            let cur = src_usdt.load(Ordering::Relaxed);
            if cur < amount_usdt_micro {
                return Err(SwapError::InsufficientBalance);
            }
            if src_usdt
                .compare_exchange_weak(
                    cur,
                    cur - amount_usdt_micro,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }

        loop {
            let cur = dst_eurc.load(Ordering::Relaxed);
            if dst_eurc
                .compare_exchange_weak(cur, cur + eurc_amount, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        Ok(eurc_amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_transfers_value() {
        let ledger = RamanLedger::new(4, 1_000_000_000_000, 0);
        let got = ledger
            .execute_jit_dex_swap(0, 1, 1_000_000, 1_085_000)
            .unwrap();
        assert_eq!(got, 1_085_000);
        assert_eq!(ledger.usdt_balance(0).unwrap(), 999_999_000_000);
        assert_eq!(ledger.eurc_balance(1).unwrap(), 1_085_000);
    }

    #[test]
    fn route_balances_only_active_avatars() {
        let ledger = RamanLedger::new(2, 0, 0);
        let avatar_a = CodegotechAvatar {
            avatar_id: Uuid::new_v4(),
            api_key: "a".to_string(),
            is_active: true,
        };
        let avatar_b = CodegotechAvatar {
            avatar_id: Uuid::new_v4(),
            api_key: "b".to_string(),
            is_active: false,
        };
        let avatar_c = CodegotechAvatar {
            avatar_id: Uuid::new_v4(),
            api_key: "c".to_string(),
            is_active: true,
        };
        ledger.replace_avatars(vec![avatar_a.clone(), avatar_b, avatar_c.clone()]);

        assert_eq!(ledger.route_via_optimal_avatar(), Some(avatar_a));
        assert_eq!(ledger.route_via_optimal_avatar(), Some(avatar_c));
        assert_eq!(ledger.route_via_optimal_avatar(), Some(avatar_a));
    }
}
