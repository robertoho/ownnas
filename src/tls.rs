//! Generate a private CA and a server certificate for a single OwnNAS installation.
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use std::io::Write;
use std::path::Path;

pub fn generate(out: &Path, hosts: Vec<String>) -> Result<(), String> {
    if hosts.is_empty()
        || hosts
            .iter()
            .any(|h| h.trim().is_empty() || h.contains("://") || h.contains('/'))
    {
        return Err(
            "Supply at least one DNS name or IP address with --host (without scheme or port)."
                .into(),
        );
    }
    let server_key = KeyPair::generate().map_err(|e| e.to_string())?;
    let mut server = CertificateParams::new(hosts).map_err(|e| e.to_string())?;
    server
        .distinguished_name
        .push(DnType::CommonName, "OwnNAS server");
    server.use_authority_key_identifier_extension = true;
    server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    // Short-lived server certificates work with modern browser lifetime limits.
    let now = std::time::SystemTime::now();
    server.not_before = (now - std::time::Duration::from_secs(300)).into();
    server.not_after = (now + std::time::Duration::from_secs(365 * 86400)).into();
    let mut ca = CertificateParams::default();
    ca.distinguished_name
        .push(DnType::CommonName, "OwnNAS private CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.not_before = server.not_before;
    ca.not_after = (now + std::time::Duration::from_secs(3650 * 86400)).into();
    let issuer = CertifiedIssuer::self_signed(ca, KeyPair::generate().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let cert = server
        .signed_by(&server_key, &issuer)
        .map_err(|e| e.to_string())?;
    // Never replace an existing installation's trust anchor or private keys.
    std::fs::create_dir(out).map_err(|e| {
        format!(
            "Could not create new certificate directory {}: {e}",
            out.display()
        )
    })?;
    write_file(
        &out.join("ca.key"),
        issuer.key().serialize_pem().as_bytes(),
        true,
    )?;
    write_file(
        &out.join("server.key"),
        server_key.serialize_pem().as_bytes(),
        true,
    )?;
    write_file(&out.join("ca.crt"), issuer.pem().as_bytes(), false)?;
    write_file(&out.join("ca.cer"), issuer.der().as_ref(), false)?;
    write_file(&out.join("server.crt"), cert.pem().as_bytes(), false)?;
    println!("Certificates written to {}", out.display());
    println!("Users install ca.crt (PEM) or ca.cer (DER) as a trusted root certificate.");
    println!("Keep ca.key and server.key private. Server certificate expires in one year.");
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8], secret: bool) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut file = options
        .open(path)
        .map_err(|e| format!("Could not create {}: {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("Could not write {}: {e}", path.display()))
}
