//! The sign-in token at rest: encrypted for this Windows user (DPAPI), so another user of the
//! computer, or a copy of the settings file, cannot use it.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: Windows allocated `cbData` bytes at `pbData`; they are copied, then freed.
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(HLOCAL(blob.pbData as _));
    }
    bytes
}

/// The token encrypted for this user, in hex.
pub fn protect(token: &str) -> Result<String, String> {
    let input = CRYPT_INTEGER_BLOB { cbData: token.len() as u32, pbData: token.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `token`, alive for the call; Windows fills `output`.
    unsafe { CryptProtectData(&input, PCWSTR::null(), None, None, None, 0, &mut output) }.map_err(|err| format!("Could not encrypt the token: {err}"))?;
    Ok(take(output).iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The token back from `protect`'s hex.
pub fn unprotect(hex: &str) -> Result<String, String> {
    let mut bytes = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2).unwrap_or(""), 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| "The saved token is damaged: sign in again".to_string())?;
    let input = CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_mut_ptr() };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe { CryptUnprotectData(&input, None, None, None, None, 0, &mut output) }
        .map_err(|err| format!("Could not read the saved token (signed in as another Windows user?): {err}"))?;
    String::from_utf8(take(output)).map_err(|_| "The saved token is damaged: sign in again".to_string())
}
