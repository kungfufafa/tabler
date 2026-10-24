//! What the drivers of key-value, document and search stores have in common with each other
//! rather than with SQL sessions.

/// Whether a driver call only reads (allowed on a read-only connection) or may change something.
/// A driver answers `Write` for any call it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Read,
    Write,
}
