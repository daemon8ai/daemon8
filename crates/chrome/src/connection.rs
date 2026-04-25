// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connecting => write!(f, "connecting"),
            Self::Connected => write!(f, "connected"),
            Self::Reconnecting => write!(f, "reconnecting"),
        }
    }
}

#[derive(Clone)]
pub struct ConnectionStatus {
    tx: Arc<watch::Sender<ConnectionState>>,
}

impl ConnectionStatus {
    pub fn new() -> (Self, watch::Receiver<ConnectionState>) {
        let (tx, rx) = watch::channel(ConnectionState::Disconnected);
        (Self { tx: Arc::new(tx) }, rx)
    }

    pub fn transition(&self, new_state: ConnectionState) {
        let old = *self.tx.borrow();
        if old != new_state {
            tracing::info!(from = %old, to = %new_state, "connection state change");
            let _ = self.tx.send(new_state);
        }
    }

    pub fn current(&self) -> ConnectionState {
        *self.tx.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_disconnected() {
        let (status, rx) = ConnectionStatus::new();
        assert_eq!(status.current(), ConnectionState::Disconnected);
        assert_eq!(*rx.borrow(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_transition_updates_all_receivers() {
        let (status, rx1) = ConnectionStatus::new();
        let rx2 = rx1.clone();

        status.transition(ConnectionState::Connected);
        assert_eq!(*rx1.borrow(), ConnectionState::Connected);
        assert_eq!(*rx2.borrow(), ConnectionState::Connected);
    }

    #[test]
    fn test_rapid_transitions_receiver_sees_latest() {
        let (status, rx) = ConnectionStatus::new();

        status.transition(ConnectionState::Connecting);
        status.transition(ConnectionState::Connected);
        status.transition(ConnectionState::Reconnecting);
        status.transition(ConnectionState::Disconnected);

        assert_eq!(*rx.borrow(), ConnectionState::Disconnected);
    }
}
