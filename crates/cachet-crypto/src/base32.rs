//! The nix base32 encoding (the alphabet lives in cachet-core's
//! constants). This is a bit-packing dialect, not RFC 4648: output
//! characters enumerate 5-bit groups from the LOW end of the byte string,
//! so the string's first character covers the highest-addressed bits and
//! its last character covers the lowest. The fixture hashes in the golden
//! lane (a real `nix hash` output and a real signed narinfo) lock this
//! dialect; an RFC-shaped implementation silently corrupts store paths.

use cachet_core::constants::NIX_BASE32_ALPHABET;

const DIGITS: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";
// why: the hardcoded digits and the shared alphabet must agree; slice ==
// is not const-stable, so the check is a loop.
const _: () = {
    let (a, b) = (DIGITS, NIX_BASE32_ALPHABET.as_bytes());
    let mut i = 0;
    while i < 32 {
        assert!(a[i] == b[i]);
        i += 1;
    }
};

/// The encoded length for an input: one output char per 5 input bits,
/// ceiling to cover the tail group.
pub const fn encoded_len(input_len: usize) -> usize {
    (input_len * 8).div_ceil(5)
}

/// Encode bytes into nix base32.
///
/// For each output position `n` counted DOWN from the end: bit offset
/// `5n`, byte `i = 5n / 8`, shift `j = 5n % 8`; the character takes the
/// five bits starting at bit `5n` of the string, adding the next byte's
/// low bits across the boundary. Iterating downward makes position 0 the
/// low group: matching nix exactly, including its orientation.
pub fn encode(data: &[u8]) -> String {
    let out_len = encoded_len(data.len());
    let mut out = String::with_capacity(out_len);
    for n in (0..out_len).rev() {
        let bit = n * 5;
        let (i, j) = (bit / 8, bit % 8);
        let mut group = u16::from(data[i] >> j);
        if i + 1 < data.len() && j > 0 {
            group |= u16::from(data[i + 1]) << (8 - j);
        }
        out.push(char::from(DIGITS[(group & 0x1f) as usize]));
    }
    out
}

/// Why a decode failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The input carried a character outside the nix alphabet.
    InvalidDigit(char),
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidDigit(c) => write!(f, "invalid nix base32 digit {c:?}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decode a nix base32 string back to bytes. Output size follows the data:
/// `floor(len * 5 / 8)` bytes plus spill bits when the packing needs them.
///
/// # Errors
///
/// [`DecodeError::InvalidDigit`] on any character outside the alphabet.
pub fn decode(text: &str) -> Result<Vec<u8>, DecodeError> {
    let mut out: Vec<u8> = Vec::with_capacity(text.len() * 5 / 8);
    for (pos, n) in (0..text.len()).rev().enumerate() {
        let bit = n * 5;
        let (i, j) = (bit / 8, bit % 8);
        // why: the digit is an index into a 32-entry alphabet, so the
        // narrowing conversions below cannot truncate anything real; the
        // try_from keeps the claim explicit.
        let digit = u8::try_from(
            DIGITS
                .iter()
                .position(|&d| d == text.as_bytes()[pos])
                .ok_or(DecodeError::InvalidDigit(text.as_bytes()[pos] as char))?,
        )
        .expect("an index into a 32-entry alphabet");
        while out.len() <= i {
            out.push(0);
        }
        out[i] |= ((u32::from(digit) << j) & 0xff) as u8;
        let spill = u8::try_from(u32::from(digit) >> (8 - j)).expect("spill < 32");
        if spill > 0 {
            while out.len() <= i + 1 {
                out.push(0);
            }
            out[i + 1] |= spill;
        }
    }
    Ok(out)
}

/// `sha256:<nix32>` of the given bytes: the way hashes appear inside a
/// narinfo.
pub fn sha256_nix32_text(bytes: &[u8]) -> String {
    format!("sha256:{}", encode(&crate::sha256::sha256(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_file_hash_vector() {
        let data = hex::decode("de04bbe5f74e4d1758b2e82e31b8bb467786b9a99659707145ecb661ed109d86")
            .expect("hex");
        assert_eq!(
            encode(&data),
            "11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y"
        );
    }

    #[test]
    fn store_hash_round_trip() {
        let hash = "qvqa04f0m85m0a6xxnan5vxnwg2jkgl9";
        assert_eq!(encode(&decode(hash).expect("decodes")), hash);
    }

    #[test]
    fn decode_refuses_foreign_digits() {
        assert!(decode("abc efg").is_err());
        assert!(decode("abco").is_err()); // 'o' is not in the alphabet
    }
}
