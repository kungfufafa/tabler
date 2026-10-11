//! SSH tunnels (russh): local listener on 127.0.0.1 forwarding to the database host through an
//! SSH server. Implemented in M6; this stub fixes the names other modules depend on.

/// Keeps a tunnel alive; dropping it (or calling `close`) shuts the listener and SSH session down.
#[derive(Debug)]
pub struct TunnelHandle {
    pub local_port: u16,
}

impl TunnelHandle {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub async fn close(&self) {}
}

/// A host key the user has not trusted yet, kept until they accept or reject it.
#[derive(Debug, Clone)]
pub struct PendingHostKey {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub public_key: String,
    pub fingerprint: String,
}
