//! AWS request signing and credentials (DynamoDB, OpenSearch Service).

pub mod credentials;
pub mod sigv4;

pub use credentials::{CredentialProvider, Credentials};
pub use sigv4::Signer;
