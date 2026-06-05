//! Kernel-verified peer credentials (SO_PEERCRED) for the credd Unix socket.
//!
//! `PeerIdentity` is captured per Unix connection from `UnixStream::peer_cred()`
//! and used to authorize local-only endpoints: only a process running as the
//! same effective user as the daemon may use them. No key material is involved
//! -- the kernel vouches for the peer UID.

/// Kernel-verified identity of a process connected over the credd Unix socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    /// Effective UID of the connecting process, from SO_PEERCRED.
    pub uid: u32,
    /// PID of the connecting process (advisory; for audit only).
    pub pid: i32,
}

impl PeerIdentity {
    /// True when the peer runs as the same effective user as this daemon.
    pub fn is_local_owner(&self) -> bool {
        self.uid == nix::unistd::geteuid().as_raw()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_uid_is_authorized() {
        let me = nix::unistd::geteuid().as_raw();
        let id = PeerIdentity { uid: me, pid: 1234 };
        assert!(id.is_local_owner());
    }
    #[test]
    fn other_uid_is_rejected() {
        // u32::MAX is never a real local owner.
        let id = PeerIdentity {
            uid: u32::MAX,
            pid: 1234,
        };
        assert!(!id.is_local_owner());
    }
}
