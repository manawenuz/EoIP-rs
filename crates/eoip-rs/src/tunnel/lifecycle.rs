//! Tunnel lifecycle state machine.
//!
//! States: Initializing → Configured → Active → Stale → TearingDown → Destroyed
//!
//! State is backed by `AtomicU8` for lock-free reads on the hot path.

use std::sync::atomic::{AtomicU8, Ordering};

/// Tunnel lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TunnelState {
    /// Resources being allocated (TAP fd requested from helper).
    Initializing = 0,
    /// TAP fd received, ready to activate.
    Configured = 1,
    /// Actively forwarding packets.
    Active = 2,
    /// Keepalive timeout expired. TX suspended, RX still active for recovery.
    Stale = 3,
    /// Shutdown in progress, draining in-flight packets.
    TearingDown = 4,
    /// Fully cleaned up, can be removed from registry.
    Destroyed = 5,
}

impl TunnelState {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Initializing),
            1 => Some(Self::Configured),
            2 => Some(Self::Active),
            3 => Some(Self::Stale),
            4 => Some(Self::TearingDown),
            5 => Some(Self::Destroyed),
            _ => None,
        }
    }
}

/// Validate whether a state transition is allowed by the FSM.
pub fn is_valid_transition(from: TunnelState, to: TunnelState) -> bool {
    use TunnelState::*;
    matches!(
        (from, to),
        (Initializing, Configured)
            | (Configured, Active)
            | (Active, Stale)
            | (Stale, Active)           // recovery on keepalive received
            | (Active, TearingDown)
            | (Stale, TearingDown)
            | (Configured, TearingDown) // shutdown before activation
            | (Initializing, TearingDown) // shutdown during init
            | (TearingDown, Destroyed)
    )
}

/// Atomic tunnel state holder for lock-free access.
pub struct AtomicTunnelState(AtomicU8);

impl AtomicTunnelState {
    pub fn new(state: TunnelState) -> Self {
        Self(AtomicU8::new(state as u8))
    }

    pub fn load(&self) -> TunnelState {
        TunnelState::from_u8(self.0.load(Ordering::Acquire))
            .unwrap_or(TunnelState::Destroyed)
    }

    /// Attempt a state transition. Returns `Ok(())` if the transition is valid
    /// and the CAS succeeds, `Err(current_state)` otherwise.
    pub fn transition(&self, from: TunnelState, to: TunnelState) -> Result<(), TunnelState> {
        if !is_valid_transition(from, to) {
            return Err(self.load());
        }

        self.0
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|actual| TunnelState::from_u8(actual).unwrap_or(TunnelState::Destroyed))
    }
}

impl std::fmt::Debug for AtomicTunnelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AtomicTunnelState({:?})", self.load())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_states() {
        for v in 0..=5u8 {
            let state = TunnelState::from_u8(v).unwrap();
            assert_eq!(state as u8, v);
        }
        assert!(TunnelState::from_u8(6).is_none());
        assert!(TunnelState::from_u8(255).is_none());
    }

    #[test]
    fn valid_transitions() {
        use TunnelState::*;
        let valid = [
            (Initializing, Configured),
            (Configured, Active),
            (Active, Stale),
            (Stale, Active),
            (Active, TearingDown),
            (Stale, TearingDown),
            (Configured, TearingDown),
            (Initializing, TearingDown),
            (TearingDown, Destroyed),
        ];
        for (from, to) in valid {
            assert!(
                is_valid_transition(from, to),
                "{from:?} -> {to:?} should be valid"
            );
        }
    }

    #[test]
    fn invalid_transitions() {
        use TunnelState::*;
        let invalid = [
            (Destroyed, Active),
            (Destroyed, Initializing),
            (Active, Configured),
            (Active, Initializing),
            (Stale, Configured),
            (Configured, Stale),
            (Initializing, Active),
            (Initializing, Stale),
        ];
        for (from, to) in invalid {
            assert!(
                !is_valid_transition(from, to),
                "{from:?} -> {to:?} should be invalid"
            );
        }
    }

    #[test]
    fn atomic_state_transition() {
        let state = AtomicTunnelState::new(TunnelState::Initializing);
        assert_eq!(state.load(), TunnelState::Initializing);

        // Valid transition
        state.transition(TunnelState::Initializing, TunnelState::Configured).unwrap();
        assert_eq!(state.load(), TunnelState::Configured);

        // Invalid transition (skip Active)
        let err = state
            .transition(TunnelState::Configured, TunnelState::Stale)
            .unwrap_err();
        assert_eq!(err, TunnelState::Configured);

        // CAS failure (wrong from state)
        let err = state
            .transition(TunnelState::Initializing, TunnelState::Configured)
            .unwrap_err();
        assert_eq!(err, TunnelState::Configured);
    }

    #[test]
    fn full_lifecycle() {
        let state = AtomicTunnelState::new(TunnelState::Initializing);
        state.transition(TunnelState::Initializing, TunnelState::Configured).unwrap();
        state.transition(TunnelState::Configured, TunnelState::Active).unwrap();
        state.transition(TunnelState::Active, TunnelState::Stale).unwrap();
        state.transition(TunnelState::Stale, TunnelState::Active).unwrap(); // recovery
        state.transition(TunnelState::Active, TunnelState::TearingDown).unwrap();
        state.transition(TunnelState::TearingDown, TunnelState::Destroyed).unwrap();
        assert_eq!(state.load(), TunnelState::Destroyed);
    }
}
