use super::security;
use crate::{VncError, VncVersion};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum SecurityType {
    Invalid = 0,
    None = 1,
    VncAuth = 2,
    RA2 = 5,
    RA2ne = 6,
    Tight = 16,
    Ultra = 17,
    Tls = 18,
    VeNCrypt = 19,
    GtkVncSasl = 20,
    Md5Hash = 21,
    ColinDeanXvp = 22,
}

impl TryFrom<u8> for SecurityType {
    type Error = VncError;
    fn try_from(num: u8) -> Result<Self, Self::Error> {
        match num {
            0 | 1 | 2 | 5 | 6 | 16 | 17 | 18 | 19 | 20 | 21 | 22 => {
                Ok(unsafe { std::mem::transmute::<u8, SecurityType>(num) })
            }
            invalid => Err(VncError::InvalidSecurityTyep(invalid)),
        }
    }
}

impl From<SecurityType> for u8 {
    fn from(e: SecurityType) -> Self {
        e as u8
    }
}

impl SecurityType {
    pub(super) async fn read<S>(reader: &mut S, version: &VncVersion) -> Result<Vec<Self>, VncError>
    where
        S: AsyncRead + Unpin,
    {
        match version {
            VncVersion::RFB33 => {
                let security_type = reader.read_u32().await?;
                let security_type = u8::try_from(security_type)
                    .map_err(|_| VncError::General(format!("Unsupported RFB 3.3 security type: {security_type}")))?
                    .try_into()?;
                if let SecurityType::Invalid = security_type {
                    let _ = reader.read_u32().await?;
                    let mut err_msg = String::new();
                    reader.read_to_string(&mut err_msg).await?;
                    return Err(VncError::General(err_msg));
                }
                Ok(vec![security_type])
            }
            _ => {
                // +--------------------------+-------------+--------------------------+
                // | No. of bytes             | Type        | Description              |
                // |                          | [Value]     |                          |
                // +--------------------------+-------------+--------------------------+
                // | 1                        | U8          | number-of-security-types |
                // | number-of-security-types | U8 array    | security-types           |
                // +--------------------------+-------------+--------------------------+
                let num = reader.read_u8().await?;

                if num == 0 {
                    let _ = reader.read_u32().await?;
                    let mut err_msg = String::new();
                    reader.read_to_string(&mut err_msg).await?;
                    return Err(VncError::General(err_msg));
                }
                let mut offered = vec![0; num as usize];
                reader.read_exact(&mut offered).await?;
                let sec_types: Vec<Self> = offered.iter().copied()
                    .filter_map(|value| Self::try_from(value).ok()).collect();
                if !sec_types.contains(&Self::None) && !sec_types.contains(&Self::VncAuth) {
                    return Err(VncError::General(format!(
                        "No supported VNC authentication method. Server offered security types {offered:?}; this client supports None (1) and VNC password (2)"
                    )));
                }
                tracing::trace!("Server supported security type: {:?}", sec_types);
                Ok(sec_types)
            }
        }
    }

    pub(super) async fn write<S>(&self, writer: &mut S) -> Result<(), VncError>
    where
        S: AsyncWrite + Unpin,
    {
        writer.write_all(&[(*self).into()]).await?;
        Ok(())
    }
}

#[allow(dead_code)]
#[repr(u32)]
pub(super) enum AuthResult {
    Ok = 0,
    Failed = 1,
}

impl From<u32> for AuthResult {
    fn from(num: u32) -> Self {
        if num == 0 { Self::Ok } else { Self::Failed }
    }
}

impl From<AuthResult> for u32 {
    fn from(e: AuthResult) -> Self {
        e as u32
    }
}

pub(super) struct AuthHelper {
    challenge: [u8; 16],
    key: [u8; 8],
}

impl AuthHelper {
    pub(super) async fn read<S>(reader: &mut S, credential: &str) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let mut challenge = [0; 16];
        reader.read_exact(&mut challenge).await?;

        let credential_len = credential.len();
        let mut key = [0u8; 8];
        for (i, key_i) in key.iter_mut().enumerate() {
            let c = if i < credential_len {
                credential.as_bytes()[i]
            } else {
                0
            };
            let mut cs = 0u8;
            for j in 0..8 {
                cs |= ((c >> j) & 1) << (7 - j)
            }
            *key_i = cs;
        }

        Ok(Self { challenge, key })
    }

    pub(super) async fn write<S>(&self, writer: &mut S) -> Result<(), VncError>
    where
        S: AsyncWrite + Unpin,
    {
        let encrypted = security::des::encrypt(&self.challenge, &self.key);
        writer.write_all(&encrypted).await?;
        Ok(())
    }

    pub(super) async fn finish<S>(self, reader: &mut S) -> Result<AuthResult, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let result = reader.read_u32().await?;
        Ok(result.into())
    }
}

#[cfg(test)]
mod negotiation_tests {
    use super::*;
    #[test]
    fn unknown_offers_do_not_hide_supported_authentication() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            for wire in [&[3,129,2,1][..], &[3,2,129,1][..], &[3,2,1,129][..]] {
                let mut input = wire;
                let types = SecurityType::read(&mut input, &VncVersion::RFB38).await.unwrap();
                assert!(types.contains(&SecurityType::VncAuth));
                assert!(types.contains(&SecurityType::None));
                assert!(input.is_empty());
            }
            let error = SecurityType::read(&mut &[2,129,19][..], &VncVersion::RFB38).await.unwrap_err();
            assert!(error.to_string().contains("[129, 19]"));
            assert!(SecurityType::read(&mut &[2,129][..], &VncVersion::RFB38).await.is_err());
            assert!(SecurityType::read(&mut &[0,0,1,2][..], &VncVersion::RFB33).await.is_err());
            assert!(matches!(AuthResult::from(2), AuthResult::Failed));
        });
    }
}
