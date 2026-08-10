use crate::SerializeError;
use data_encoding::BASE32_NOPAD;
use serde::Serialize;

pub(crate) fn canonical_json<T: Serialize>(value: &T) -> Result<String, SerializeError> {
    Ok(serde_json_canonicalizer::to_string(value)?)
}

pub(crate) fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json_canonicalizer::to_vec(value)
}

pub(crate) fn signature<T: Serialize>(value: &T) -> Result<[u8; 32], serde_json::Error> {
    Ok(*blake3::hash(&canonical_bytes(value)?).as_bytes())
}

pub(crate) fn signature_hex<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(blake3::hash(&canonical_bytes(value)?).to_hex().to_string())
}

pub(crate) fn short_id(prefix: &str, signature: &[u8; 32]) -> String {
    format!(
        "{prefix}{}",
        BASE32_NOPAD.encode(&signature[..16]).to_ascii_lowercase()
    )
}
