//! TLS utilities for building reqwest clients with bundled root certificates.
//!
//! On Android/Termux, `rustls-platform-verifier` requires JNI initialization
//! which is unavailable outside a full Android app. Using `tls_backend_preconfigured()`
//! with a custom `rustls::ClientConfig` avoids the platform verifier entirely.

/// Build a `reqwest::Client` that uses Mozilla's bundled root certificates
/// instead of `rustls-platform-verifier`.
///
/// On Android (Termux), `rustls-platform-verifier` panics because it needs a
/// JNI environment that doesn't exist outside of an Android app. This function
/// constructs a `rustls::ClientConfig` with the bundled root certificates from
/// `webpki_root_certs`, bypassing the platform verifier entirely.
pub fn reqwest_client() -> reqwest::Client {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .build()
        .expect("Failed to build reqwest Client with custom TLS config")
}

/// Return a shared `reqwest::blocking::Client` with bundled root certificates
/// (no platform verifier). Created once via `OnceLock` so its internal tokio
/// runtime is never dropped inside an async context.
pub fn blocking_reqwest_client() -> &'static reqwest::blocking::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut root_store = rustls::RootCertStore::empty();
        root_store
            .add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(config)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build blocking reqwest Client with custom TLS config")
    })
}
