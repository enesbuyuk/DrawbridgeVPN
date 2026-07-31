#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Idle,
    LoggingIn,
    WaitingMfa,
    ConnectingVpn,
    Connected,
    Failed,
}
impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            ConnectionStatus::Idle => "Idle",
            ConnectionStatus::LoggingIn => "Logging In",
            ConnectionStatus::WaitingMfa => "Waiting for MFA",
            ConnectionStatus::ConnectingVpn => "Connecting VPN",
            ConnectionStatus::Connected => "Connected",
            ConnectionStatus::Failed => "Failed",
        };
        write!(f, "{label}")
    }
}
#[derive(Debug, Clone)]
pub enum LogEvent {
    Status(ConnectionStatus),
    Log(String),
    Error(String),
    Connected(u32),
    Disconnected,
    NetSpeed { down_bps: f64, up_bps: f64 },
}
