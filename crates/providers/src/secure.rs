//! Armazenamento de segredos com DPAPI: só o mesmo usuário do Windows consegue decifrar.
use windows::core::w;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

/// Entropia extra: o blob só abre com este app (além de exigir o mesmo usuário).
const ENTROPY: &[u8] = b"line-quota/claude-auth/v1";

fn blob(d: &[u8]) -> CRYPT_INTEGER_BLOB { CRYPT_INTEGER_BLOB { cbData: d.len() as u32, pbData: d.as_ptr() as *mut u8 } }

unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    let _ = LocalFree(HLOCAL(out.pbData as *mut _));
    v
}

pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    let (input, ent) = (blob(data), blob(ENTROPY));
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, w!("LineQuota"), Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            .map_err(|_| "falha ao criptografar (DPAPI)".to_string())?;
        Ok(take(out))
    }
}

pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    let (input, ent) = (blob(data), blob(ENTROPY));
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            .map_err(|_| "não foi possível decifrar o token salvo".to_string())?;
        Ok(take(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_not_plaintext() {
        let secret = b"token-super-secreto-123";
        let enc = protect(secret).unwrap();
        assert!(!enc.windows(secret.len()).any(|w| w == secret));
        assert_eq!(unprotect(&enc).unwrap(), secret);
    }

    #[test]
    fn tampered_blob_fails() {
        let mut enc = protect(b"abc").unwrap();
        let n = enc.len() - 1;
        enc[n] ^= 0xFF;
        assert!(unprotect(&enc).is_err());
    }
}
