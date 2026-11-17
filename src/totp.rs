//! Two-factor sign-in: six-digit codes from an authenticator app (TOTP, RFC 6238: HMAC-SHA1,
//! 30-second steps), and one-time recovery codes for when the phone is lost.

use hmac::{Hmac, Mac};
use rand::Rng;

/// A code is good for its own 30 seconds and the ones just before and after (clocks drift).
const STEP: i64 = 30;
const DIGITS: u32 = 6;
const RECOVERY_CODES: usize = 10;
/// No 0/o, 1/l/i: recovery codes are read off paper.
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A new secret, 160 random bits written in base32 as authenticator apps expect.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill(&mut bytes);
    base32_encode(&bytes)
}

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            out.push(BASE32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let value = BASE32.iter().position(|&b| b as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// The code for one 30-second step.
fn code_at(key: &[u8], step: i64) -> u32 {
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(key).expect("any key length works");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let value = u32::from_be_bytes([digest[offset] & 0x7f, digest[offset + 1], digest[offset + 2], digest[offset + 3]]);
    value % 10u32.pow(DIGITS)
}

/// The step of a code that is right now, if `code` is one. A step at or before `used` was
/// already used to sign in, and does not count again.
pub fn check(secret: &str, code: &str, now: i64, used: i64) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let wanted: u32 = code.parse().ok()?;
    let key = base32_decode(secret)?;
    let current = now.div_euclid(STEP);
    // Compared in full each time, so how long it takes does not tell which step matched.
    let mut found = None;
    for step in [current - 1, current, current + 1] {
        if code_at(&key, step) == wanted && step > used && found.is_none() {
            found = Some(step);
        }
    }
    found
}

/// What authenticator apps scan: `otpauth://totp/OwnNAS:alice?secret=…&issuer=OwnNAS`.
pub fn uri(issuer: &str, account: &str, secret: &str) -> String {
    let label = format!("{}:{}", encode(issuer), encode(account));
    format!("otpauth://totp/{label}?secret={secret}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP}", encode(issuer))
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// The URI as a QR code, an SVG drawn in the page's text colour.
pub fn qr_svg(uri: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(uri.as_bytes(), qrcode::EcLevel::M).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .dark_color(qrcode::render::svg::Color("#000000"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build(),
    )
}

/// Ten new recovery codes such as `k7dm-29xq`, each good for one sign-in.
pub fn new_recovery_codes() -> Vec<String> {
    let mut rng = rand::rngs::OsRng;
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut part = || (0..4).map(|_| RECOVERY_ALPHABET[rng.gen_range(0..RECOVERY_ALPHABET.len())] as char).collect::<String>();
            format!("{}-{}", part(), part())
        })
        .collect()
}

/// A recovery code as typed: lower case, without spaces, with the dash in its place.
pub fn normalize_recovery(code: &str) -> Option<String> {
    let letters: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect();
    (letters.len() == 8).then(|| format!("{}-{}", &letters[..4], &letters[4..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_6238_codes() {
        // RFC 6238's SHA1 test key, "12345678901234567890", and its published codes (last six digits).
        let secret = base32_encode(b"12345678901234567890");
        assert_eq!(secret, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        let key = base32_decode(&secret).unwrap();
        assert_eq!(code_at(&key, 59 / 30), 287082);
        assert_eq!(code_at(&key, 1111111109 / 30), 81804);
        assert_eq!(code_at(&key, 1234567890 / 30), 5924);
        assert_eq!(code_at(&key, 2000000000 / 30), 279037);
    }

    #[test]
    fn checking_codes() {
        let secret = new_secret();
        let key = base32_decode(&secret).unwrap();
        let now = 1_800_000_000;
        let code = format!("{:06}", code_at(&key, now / 30));
        assert_eq!(check(&secret, &code, now, 0), Some(now / 30));
        assert_eq!(check(&secret, &format!("{} {}", &code[..3], &code[3..]), now, 0), Some(now / 30), "spaces are fine");
        assert_eq!(check(&secret, &code, now + 30, 0), Some(now / 30), "the step before still counts");
        assert_eq!(check(&secret, &code, now + 90, 0), None, "but not older ones");
        assert_eq!(check(&secret, &code, now, now / 30), None, "a code signs in once");
        assert_eq!(check(&secret, "12345", now, 0), None);
        assert_eq!(check(&secret, "abcdef", now, 0), None);
    }

    #[test]
    fn uris_and_recovery_codes() {
        let uri = uri("OwnNAS", "ann smith", "ABC");
        assert!(uri.starts_with("otpauth://totp/OwnNAS:ann%20smith?secret=ABC&issuer=OwnNAS"), "{uri}");
        assert!(qr_svg(&uri).unwrap().starts_with("<?xml"));
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        assert!(codes.iter().all(|code| code.len() == 9 && code.as_bytes()[4] == b'-'));
        assert_eq!(normalize_recovery(" K7DM 29XQ "), Some("k7dm-29xq".into()));
        assert_eq!(normalize_recovery("123456"), None);
    }
}
