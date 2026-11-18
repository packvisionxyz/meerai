use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

pub(crate) fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0u8; len];
    getrandom::fill(&mut buffer)
        .map_err(|err| anyhow::anyhow!("failed to read random bytes: {err}"))?;
    Ok(buffer)
}

pub(crate) fn random_token() -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes(32)?))
}

pub(crate) fn code_verifier() -> Result<String> {
    random_token()
}

pub(crate) fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub(crate) fn uuid_v4() -> Result<String> {
    let mut bytes = random_bytes(16)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_the_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifiers_are_url_safe_and_long_enough() {
        let first = code_verifier().unwrap();
        let second = code_verifier().unwrap();
        assert_ne!(first, second);
        assert!(first.len() >= 43);
        assert!(first.len() <= 128);
        assert!(
            first
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
        assert_eq!(URL_SAFE_NO_PAD.decode(&first).unwrap().len(), 32);
        assert_eq!(challenge_for(&first).len(), 43);
    }

    #[test]
    fn uuid_v4_has_the_expected_shape() {
        let value = uuid_v4().unwrap();
        assert_eq!(value.len(), 36);
        assert_eq!(value.as_bytes()[8], b'-');
        assert_eq!(value.as_bytes()[13], b'-');
        assert_eq!(value.as_bytes()[14], b'4');
        assert_eq!(value.as_bytes()[18], b'-');
        assert_eq!(value.as_bytes()[23], b'-');
        assert!(matches!(value.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }
}
