//! Patched `rustls-platform-verifier` that uses `webpki-root-certs` (Mozilla's
//! root CA bundle) instead of platform-specific verifiers (JNI on Android, etc.).
//!
//! This replaces the upstream crate which panics on Android/Termux due to missing JVM.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme,
    client::WantsClientCert, ClientConfig, ConfigBuilder, WantsVerifier,
};

/// A TLS certificate verifier backed by Mozilla's bundled root certificates.
#[derive(Debug)]
pub struct Verifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl Verifier {
    /// Creates a new verifier using bundled webpki root certificates.
    pub fn new(crypto_provider: Arc<CryptoProvider>) -> Result<Self, TlsError> {
        let mut root_store = rustls::RootCertStore::empty();
        Self::add_webpki_roots(&mut root_store)?;
        Self::build(root_store, crypto_provider)
    }

    /// Creates a new verifier with bundled roots plus extra root certificates.
    pub fn new_with_extra_roots(
        extra_roots: impl IntoIterator<Item = CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        let mut root_store = rustls::RootCertStore::empty();
        for cert in extra_roots {
            root_store.add(cert)?;
        }
        Self::add_webpki_roots(&mut root_store)?;
        Self::build(root_store, crypto_provider)
    }

    fn add_webpki_roots(root_store: &mut rustls::RootCertStore) -> Result<(), TlsError> {
        let (added, ignored) = root_store
            .add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());
        if ignored > 0 {
            log::warn!("{ignored} webpki root certificates were ignored due to errors");
        }
        log::debug!("Loaded {added} CA root certificates from webpki-root-certs");
        if root_store.is_empty() {
            return Err(TlsError::General(
                "No CA certificates were loaded from webpki-root-certs".into(),
            ));
        }
        Ok(())
    }

    fn build(
        root_store: rustls::RootCertStore,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        Ok(Self {
            inner: WebPkiServerVerifier::builder_with_provider(root_store.into(), crypto_provider)
                .build()
                .map_err(|e| TlsError::Other(OtherError(Arc::new(e))))?,
        })
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Extension trait to configure a [`ClientConfig`] with the bundled webpki verifier.
pub trait BuilderVerifierExt {
    /// Configures the `ClientConfig` with the bundled webpki verifier.
    fn with_platform_verifier(
        self,
    ) -> Result<ConfigBuilder<ClientConfig, WantsClientCert>, rustls::Error>;
}

impl BuilderVerifierExt for ConfigBuilder<ClientConfig, WantsVerifier> {
    fn with_platform_verifier(
        self,
    ) -> Result<ConfigBuilder<ClientConfig, WantsClientCert>, rustls::Error> {
        let verifier = Verifier::new(self.crypto_provider().clone())?;
        Ok(self
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier)))
    }
}

/// Extension trait to build a [`ClientConfig`] with the bundled webpki verifier.
pub trait ConfigVerifierExt {
    /// Build a [`ClientConfig`] with the bundled webpki verifier.
    fn with_platform_verifier() -> Result<ClientConfig, rustls::Error>;
}

impl ConfigVerifierExt for ClientConfig {
    fn with_platform_verifier() -> Result<ClientConfig, rustls::Error> {
        Ok(ClientConfig::builder()
            .with_platform_verifier()?
            .with_no_client_auth())
    }
}
