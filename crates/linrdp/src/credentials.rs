//! MS-CSSP 2.2.1.2 / 2.2.1.2.1 password delegation, UTF-16LE without NULs.
use der::{Encode, Sequence, asn1::OctetStringRef};
use zeroize::Zeroizing;

#[derive(Sequence)]
struct Password<'a> {
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT")]
    domain: OctetStringRef<'a>,
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT")]
    user: OctetStringRef<'a>,
    #[asn1(context_specific = "2", tag_mode = "EXPLICIT")]
    password: OctetStringRef<'a>,
}

#[derive(Sequence)]
struct Credentials<'a> {
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT")]
    kind: u32,
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT")]
    credentials: OctetStringRef<'a>,
}

pub fn encode(
    domain: &str,
    user: &str,
    password: &str,
) -> Result<Zeroizing<Vec<u8>>, Box<dyn std::error::Error>> {
    if [domain, user, password]
        .iter()
        .any(|v| v.len() > 65536 || v.contains('\0'))
        || user.is_empty()
    {
        return Err("invalid or oversized credential field".into());
    }
    let utf16 = |s: &str| {
        Zeroizing::new(
            s.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
    };
    let domain = utf16(domain);
    let user = utf16(user);
    let password = utf16(password);
    let inner = Password {
        domain: OctetStringRef::new(&domain)?,
        user: OctetStringRef::new(&user)?,
        password: OctetStringRef::new(&password)?,
    };
    let mut inner_bytes = Zeroizing::new(vec![0; usize::try_from(inner.encoded_len()?)?]);
    inner.encode_to_slice(&mut inner_bytes)?;
    let outer = Credentials {
        kind: 1,
        credentials: OctetStringRef::new(&inner_bytes)?,
    };
    let mut bytes = Zeroizing::new(vec![0; usize::try_from(outer.encoded_len()?)?]);
    outer.encode_to_slice(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use der::Decode;

    #[test]
    fn matches_independent_password_credentials_vector() {
        let bytes = encode("D", "U", "P").unwrap();
        assert_eq!(
            &*bytes,
            &[
                0x30, 0x1d, 0xa0, 3, 2, 1, 1, 0xa1, 0x16, 4, 0x14, 0x30, 0x12, 0xa0, 4, 4, 2, b'D',
                0, 0xa1, 4, 4, 2, b'U', 0, 0xa2, 4, 4, 2, b'P', 0,
            ]
        );
    }

    #[test]
    fn encodes_unicode_as_utf16le_without_terminators() {
        let bytes = encode("", "æ", "🔑").unwrap();
        let outer = Credentials::from_der(&bytes).unwrap();
        let inner = Password::from_der(outer.credentials.as_bytes()).unwrap();
        assert_eq!(inner.domain.as_bytes(), &[]);
        assert_eq!(inner.user.as_bytes(), &[0xe6, 0]);
        assert_eq!(inner.password.as_bytes(), &[0x3d, 0xd8, 0x11, 0xdd]);
    }
}
