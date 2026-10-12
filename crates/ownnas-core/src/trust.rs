//! Which HTTPS certificates a client accepts: the public roots, plus optionally a private CA
//! (the `ca.crt` from `ownnas tls generate`) and certificates the user pinned by fingerprint.
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

/// Extra certificates to trust beyond the public roots.
#[derive(Clone, Debug, Default)]
pub struct Trust {
    /// PEM certificates of private CAs.
    pub ca_pem: Option<Vec<u8>>,
    /// SHA-256 fingerprints of server certificates to accept even though no root vouches for
    /// them, as `AB:CD:…` (see [`fingerprint`]).
    pub pinned: Vec<String>,
}

/// The SHA-256 of a certificate, as browsers and `ownnas serve` show it: `AB:CD:…`.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub(crate) fn client_config(trust: &Trust) -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = &trust.ca_pem {
        use rustls::pki_types::pem::PemObject;
        let mut added = 0;
        for cert in CertificateDer::pem_slice_iter(pem) {
            let cert = cert.map_err(|err| format!("Could not read the CA certificate: {err}"))?;
            roots.add(cert).map_err(|err| format!("Could not use the CA certificate: {err}"))?;
            added += 1;
        }
        if added == 0 {
            return Err("The CA certificate file has no PEM certificates".into());
        }
    }
    let provider = provider();
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?;
    let config = if trust.pinned.is_empty() {
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        let roots = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
            .build()
            .map_err(|err| err.to_string())?;
        let pinned = trust.pinned.iter().map(|pin| pin.to_ascii_uppercase()).collect();
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned { roots, pinned, provider }))
            .with_no_client_auth()
    };
    Ok(Arc::new(config))
}

/// Accepts what the roots accept, and also the exact certificates the user pinned.
#[derive(Debug)]
struct Pinned {
    roots: Arc<WebPkiServerVerifier>,
    pinned: Vec<String>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.roots.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
            Ok(verified) => Ok(verified),
            // A pin names this exact certificate, so neither its issuer nor the name it was
            // reached by matter. The handshake signatures are still checked below.
            Err(_) if self.pinned.contains(&fingerprint(end_entity)) => Ok(ServerCertVerified::assertion()),
            Err(err) => Err(err),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Records the certificate a server presents without judging it, to show the user.
#[derive(Debug)]
struct Recorder {
    seen: Mutex<Option<Vec<u8>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap_or_else(|err| err.into_inner()) = Some(end_entity.to_vec());
        // Stop here: only the certificate was wanted.
        Err(rustls::Error::General("recorded".into()))
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("recorded".into()))
    }

    fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("recorded".into()))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// The fingerprint of the certificate an `https://` server presents, to show before trusting it.
pub fn server_fingerprint(address: &str) -> Result<String, String> {
    let rest = address.strip_prefix("https://").ok_or("Only https:// addresses have certificates")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']').ok_or("Not a server address")?;
        (host.to_string(), after.strip_prefix(':').and_then(|port| port.parse().ok()).unwrap_or(443))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().map_err(|_| "Not a server address")?),
            None => (authority.to_string(), 443),
        }
    };
    let recorder = Arc::new(Recorder { seen: Mutex::new(None), provider: provider() });
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    let name = ServerName::try_from(host.clone()).map_err(|err| err.to_string())?;
    let mut connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|err| err.to_string())?;
    let address = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| err.to_string())?
        .next()
        .ok_or_else(|| format!("Could not find {host}"))?;
    let mut tcp = TcpStream::connect_timeout(&address, Duration::from_secs(15)).map_err(|err| err.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    // The handshake fails on purpose once the certificate is recorded.
    let _ = rustls::Stream::new(&mut connection, &mut tcp).write_all(b"");
    let _ = rustls::Stream::new(&mut connection, &mut tcp).read(&mut [0u8; 1]);
    let seen = recorder.seen.lock().unwrap_or_else(|err| err.into_inner()).take();
    seen.map(|der| fingerprint(&der)).ok_or_else(|| "The server did not present a certificate".into())
}

/// Whether a failed request failed because the server's certificate is not trusted.
pub(crate) fn untrusted(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = Some(err);
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            if let Some(inner) = io.get_ref() {
                if untrusted(inner) {
                    return true;
                }
            }
        }
        if matches!(err.downcast_ref::<rustls::Error>(), Some(rustls::Error::InvalidCertificate(_))) {
            return true;
        }
        source = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_look_like_browsers_show_them() {
        let print = fingerprint(b"hello");
        assert_eq!(print.len(), 32 * 3 - 1);
        assert!(print.starts_with("2C:F2:4D:BA"));
    }

    #[test]
    fn pins_switch_to_the_pinning_verifier() {
        assert!(client_config(&Trust::default()).is_ok());
        let pinned = Trust { ca_pem: None, pinned: vec!["ab:cd".into()] };
        assert!(client_config(&pinned).is_ok());
        let bad = Trust { ca_pem: Some(b"not a certificate".to_vec()), pinned: vec![] };
        assert!(client_config(&bad).is_err());
    }
}
