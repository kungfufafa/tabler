use std::sync::Arc;

use rustls::{
    client::{
        danger::{ServerCertVerified, ServerCertVerifier},
        WebPkiServerVerifier,
    },
    pki_types::{pem, CertificateDer, ServerName},
    ClientConfig, RootCertStore,
};

use crate::{io::Endpoint, Result, SslOpts, TlsError};

/// TABLER PATCH: a rustls connector, or (feature `legacy-tls`, `SslOpts::with_legacy_tls`) the
/// platform's TLS library for TLS 1.0 / 1.1 servers. Upstream re-exports
/// `tokio_rustls::TlsConnector` here.
#[derive(Clone)]
pub(crate) enum TlsConnector {
    Rustls(tokio_rustls::TlsConnector),
    #[cfg(feature = "legacy-tls")]
    Legacy(tokio_native_tls::TlsConnector),
}

impl SslOpts {
    async fn load_root_certs(&self) -> crate::Result<Vec<CertificateDer<'static>>> {
        let mut output = Vec::new();

        for root_cert in self.root_certs() {
            let root_cert_data = root_cert.read().await?;
            let mut seen = false;
            for cert in pem::SliceIter::<CertificateDer<'_>>::new(&root_cert_data) {
                seen = true;
                output.push(cert?);
            }

            if !seen && !root_cert_data.is_empty() {
                output.push(CertificateDer::from(root_cert_data.into_owned()));
            }
        }

        Ok(output)
    }

    /// TABLER PATCH: the TLS 1.0+ connector of the platform's library. Trust follows the same
    /// options as the rustls one: `accept_invalid_certs` (no verification),
    /// `skip_domain_validation` (the chain only), the root certificates added with
    /// `with_root_certs` and, unless `disable_built_in_roots`, the system's roots.
    #[cfg(feature = "legacy-tls")]
    async fn build_legacy_tls_connector(&self) -> Result<TlsConnector> {
        use tokio_native_tls::native_tls::{self, Certificate, Protocol};

        let mut builder = native_tls::TlsConnector::builder();
        builder.min_protocol_version(Some(Protocol::Tlsv10));
        for cert in self.load_root_certs().await? {
            let cert = Certificate::from_der(cert.as_ref()).map_err(|e| {
                crate::error::Error::Other(
                    format!("a root certificate cannot be used for TLS 1.0 / 1.1: {e}").into(),
                )
            })?;
            builder.add_root_certificate(cert);
        }
        builder.danger_accept_invalid_hostnames(self.skip_domain_validation());
        builder.danger_accept_invalid_certs(self.accept_invalid_certs());
        builder.disable_built_in_roots(self.disable_built_in_roots());
        let connector = builder.build().map_err(|e| {
            crate::error::Error::Other(format!("TLS 1.0 / 1.1 connector: {e}").into())
        })?;
        Ok(TlsConnector::Legacy(connector.into()))
    }

    pub(crate) async fn build_tls_connector(&self) -> Result<TlsConnector> {
        #[cfg(feature = "legacy-tls")]
        if self.legacy_tls() {
            return self.build_legacy_tls_connector().await;
        }
        let mut root_store = RootCertStore::empty();
        if !self.disable_built_in_roots() {
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().map(|x| x.to_owned()));
        }

        for cert in self.load_root_certs().await? {
            root_store.add(cert)?;
        }

        let config_builder = ClientConfig::builder().with_root_certificates(root_store.clone());

        let mut config = if let Some(identity) = self.client_identity() {
            let (cert_chain, priv_key) = identity.load().await?;
            config_builder.with_client_auth_cert(cert_chain, priv_key)?
        } else {
            config_builder.with_no_client_auth()
        };

        let mut dangerous = config.dangerous();
        let web_pki_verifier = WebPkiServerVerifier::builder(Arc::new(root_store))
            .build()
            .map_err(TlsError::from)?;
        let dangerous_verifier = DangerousVerifier::new(
            self.accept_invalid_certs(),
            self.skip_domain_validation(),
            web_pki_verifier,
        );
        dangerous.set_certificate_verifier(Arc::new(dangerous_verifier));
        let client_config = Arc::new(config);
        Ok(TlsConnector::Rustls(tokio_rustls::TlsConnector::from(
            client_config,
        )))
    }
}

impl Endpoint {
    pub async fn make_secure(
        &mut self,
        domain: String,
        tls_connector: &TlsConnector,
    ) -> Result<()> {
        #[cfg(unix)]
        if self.is_socket() {
            // won't secure socket connection
            return Ok(());
        }

        *self = match self {
            Endpoint::Plain(ref mut stream) => {
                let stream = stream.take().unwrap();

                match tls_connector {
                    TlsConnector::Rustls(tls_connector) => {
                        let server_name = ServerName::try_from(domain.as_str())
                            .map_err(TlsError::InvalidDnsName)?
                            .to_owned();
                        let connection = tls_connector.connect(server_name, stream).await?;

                        Endpoint::Secure(connection)
                    }
                    #[cfg(feature = "legacy-tls")]
                    TlsConnector::Legacy(tls_connector) => {
                        let connection = tls_connector.connect(&domain, stream).await.map_err(
                            |e| {
                                crate::error::Error::Other(
                                    format!("TLS 1.0 / 1.1 handshake: {e}").into(),
                                )
                            },
                        )?;

                        Endpoint::LegacySecure(connection)
                    }
                }
            }
            Endpoint::Secure(_) => unreachable!(),
            #[cfg(feature = "legacy-tls")]
            Endpoint::LegacySecure(_) => unreachable!(),
            #[cfg(unix)]
            Endpoint::Socket(_) => unreachable!(),
        };

        Ok(())
    }
}

#[derive(Debug)]
struct DangerousVerifier {
    accept_invalid_certs: bool,
    skip_domain_validation: bool,
    verifier: Arc<WebPkiServerVerifier>,
}

impl DangerousVerifier {
    fn new(
        accept_invalid_certs: bool,
        skip_domain_validation: bool,
        verifier: Arc<WebPkiServerVerifier>,
    ) -> Self {
        Self {
            accept_invalid_certs,
            skip_domain_validation,
            verifier,
        }
    }
}

impl ServerCertVerifier for DangerousVerifier {
    // fn verify_server_cert(
    //     &self,
    //     end_entity: &Certificate,
    //     intermediates: &[Certificate],
    //     server_name: &rustls::ServerName,
    //     scts: &mut dyn Iterator<Item = &[u8]>,
    //     ocsp_response: &[u8],
    //     now: std::time::SystemTime,
    // ) -> std::result::Result<rustls::client::ServerCertVerified, rustls::Error> {
    //     if self.accept_invalid_certs {
    //         Ok(rustls::client::ServerCertVerified::assertion())
    //     } else {
    //         match self.verifier.verify_server_cert(
    //             end_entity,
    //             intermediates,
    //             server_name,
    //             scts,
    //             ocsp_response,
    //             now,
    //         ) {
    //             Ok(assertion) => Ok(assertion),
    //             Err(ref e)
    //                 if e.to_string().contains("NotValidForName") && self.skip_domain_validation =>
    //             {
    //                 Ok(rustls::client::ServerCertVerified::assertion())
    //             }
    //             Err(e) => Err(e),
    //         }
    //     }
    // }
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::prelude::v1::Result<ServerCertVerified, rustls::Error> {
        if self.accept_invalid_certs {
            Ok(ServerCertVerified::assertion())
        } else {
            match self.verifier.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ) {
                Ok(assertion) => Ok(assertion),
                // TABLER PATCH. Upstream tests `e.to_string().contains("NotValidForName")`, but
                // rustls 0.23.x prints "certificate not valid for name ...", so the name check
                // was never skipped and `skip_domain_validation` had no effect. Match the typed
                // error instead. Chain, validity period and signature checks are untouched: only
                // the "host name is not in the certificate" verdict is waived.
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::NotValidForName
                    | rustls::CertificateError::NotValidForNameContext { .. },
                )) if self.skip_domain_validation => Ok(ServerCertVerified::assertion()),
                Err(e) => Err(e),
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::prelude::v1::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
    {
        self.verifier.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::prelude::v1::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
    {
        self.verifier.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}
