/// Why a kernel operation failed. Messages are meant for the user (they appear next to
/// the failing feature).
#[derive(Clone, Debug, PartialEq)]
pub enum KernelError {
    /// The input can't produce a solid (no closed region, zero depth, ...).
    InvalidInput(String),
    /// A geometric configuration the kernel doesn't handle yet.
    Unsupported(String),
    /// The operation would produce invalid or degenerate topology.
    InvalidResult(String),
}

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(m) | Self::InvalidResult(m) => f.write_str(m),
            Self::Unsupported(m) => write!(f, "not supported yet: {m}"),
        }
    }
}

impl std::error::Error for KernelError {}
