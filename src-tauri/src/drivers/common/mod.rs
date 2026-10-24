//! Transport several lane drivers share: an HTTP client with tunnels, token refresh and retries,
//! command-line tokens, JWT signing and AWS SigV4. Each part is built only with the features of
//! the drivers that use it.

#[cfg(feature = "common-aws")]
pub mod aws;
#[cfg(feature = "common-http")]
pub mod http;
#[cfg(feature = "common-jwt")]
pub mod jwt;
#[cfg(feature = "common-http")]
pub mod token;
