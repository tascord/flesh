#[derive(Clone, Copy, Debug)]
pub enum Status {
    /// [001] -- Announce self to network
    Announce,
    /// [002] -- Request local availability
    Ping,
    /// [003] -- Provides local availability
    Pong,
    /// [004] -- Request Key
    RequestKey,
    /// [005] -- Provide Key
    ProvideKey,
    /// [006] -- Request relay availability
    RequestRelay,
    /// [007] -- Provide relay availability
    ProvideRelay,
    /// [008] -- Relay request
    Relay,
    /// [015] -- Provided payload is too large (HTTP Equivalent 413)
    TooLarge,
    /// [016] -- Failed to receive ACK within timeframe (HTTP Equivalent 522)
    Timeout,
    /// [017] --
    RelayFailure,
    /// [021] -- Immediate hints for a long processing request (HTTP Equivalent 103)
    EarlyHints,
    /// [022] -- Hint that a path is no longer valid (HTTP Equivalent 300)
    Redirect,
    /// [031] -- Data received successfully (HTTP Equivalent 200)
    Acknowledge,
    /// [032] -- Non authorative information (fedi?) (HTTP Equivalent 203)
    NonAuthorative,
    /// [033] -- " (HTTP Equivalent 208)
    AlreadyReported,
    /// [041] -- Failed to deserialize, or unrecoverable error in processing (HTTP Equivalent 422)
    UnprocessableEntity,
    /// [042] -- Unauthorized (HTTP Equivalent 401)
    Unauthorized,
    /// [043] -- Forbidden (HTTP Equivalent 403)
    Forbidden,
    /// [044] -- Not Found (HTTP Equivalent 404)
    NotFound,
    /// [051] -- Generic hint that there was a server failure while processing (HTTP Equivalent 500)
    ServerError,
    /// [255] -- Im a teapot dude. What do you want from me (HTTP Equivalent 218)
    Teapot,
    Custom(u8),
}
impl Status {
    pub const STANDARD: [Self; 22usize] = [
        Self::Announce,
        Self::Ping,
        Self::Pong,
        Self::RequestKey,
        Self::ProvideKey,
        Self::RequestRelay,
        Self::ProvideRelay,
        Self::Relay,
        Self::TooLarge,
        Self::Timeout,
        Self::RelayFailure,
        Self::EarlyHints,
        Self::Redirect,
        Self::Acknowledge,
        Self::NonAuthorative,
        Self::AlreadyReported,
        Self::UnprocessableEntity,
        Self::Unauthorized,
        Self::Forbidden,
        Self::NotFound,
        Self::ServerError,
        Self::Teapot,
    ];

    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Announce => 1u8,
            Self::Ping => 2u8,
            Self::Pong => 3u8,
            Self::RequestKey => 4u8,
            Self::ProvideKey => 5u8,
            Self::RequestRelay => 6u8,
            Self::ProvideRelay => 7u8,
            Self::Relay => 8u8,
            Self::TooLarge => 15u8,
            Self::Timeout => 16u8,
            Self::RelayFailure => 17u8,
            Self::EarlyHints => 21u8,
            Self::Redirect => 22u8,
            Self::Acknowledge => 31u8,
            Self::NonAuthorative => 32u8,
            Self::AlreadyReported => 33u8,
            Self::UnprocessableEntity => 41u8,
            Self::Unauthorized => 42u8,
            Self::Forbidden => 43u8,
            Self::NotFound => 44u8,
            Self::ServerError => 51u8,
            Self::Teapot => 255u8,
            Self::Custom(int) => *int,
        }
    }

    pub fn as_type(&self) -> StatusType {
        match self {
            Self::Announce => StatusType::Routing,
            Self::Ping => StatusType::Routing,
            Self::Pong => StatusType::Routing,
            Self::RequestKey => StatusType::Routing,
            Self::ProvideKey => StatusType::Routing,
            Self::RequestRelay => StatusType::Routing,
            Self::ProvideRelay => StatusType::Routing,
            Self::Relay => StatusType::Routing,
            Self::TooLarge => StatusType::RoutingError,
            Self::Timeout => StatusType::RoutingError,
            Self::RelayFailure => StatusType::RoutingError,
            Self::EarlyHints => StatusType::Hints,
            Self::Redirect => StatusType::Hints,
            Self::Acknowledge => StatusType::Oks,
            Self::NonAuthorative => StatusType::Oks,
            Self::AlreadyReported => StatusType::Oks,
            Self::UnprocessableEntity => StatusType::ClientErrors,
            Self::Unauthorized => StatusType::ClientErrors,
            Self::Forbidden => StatusType::ClientErrors,
            Self::NotFound => StatusType::ClientErrors,
            Self::ServerError => StatusType::ServerErrors,
            Self::Teapot => StatusType::Oks,
            Self::Custom(_) => StatusType::Unknown,
        }
    }

    pub fn is_ok(&self) -> bool {
        match self.as_type() {
            StatusType::Routing | StatusType::Hints | StatusType::Oks => true,
            _ => false,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum StatusType {
    /// 001 -> 014
    Routing,
    /// 015 -> 020
    RoutingError,
    /// 021 -> 030
    Hints,
    /// 031 -> 040
    Oks,
    /// 041 -> 050
    ClientErrors,
    /// 051 -> 060
    ServerErrors,
    /// Currently unbound or in custom range 061->254(~)
    Unknown,
}
