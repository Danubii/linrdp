//! CredSSP TSRequest envelopes, MS-CSSP sections 2.2.1 and 2.2.1.1.
//! https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/6aac4dea-08ef-47a6-8747-22ea7f6d8685
//!
//! Only framing is implemented here. Tokens and encrypted fields are opaque.
//! These messages must travel inside verified TLS, never directly over TCP.

use der::{
    Decode, Encode, Sequence, Tagged,
    asn1::{AnyRef, OctetStringRef, SequenceOf},
};
use std::fmt;

/// Local resource policy, not a limit imposed by the wire specification.
pub const MAX_MESSAGE_SIZE: usize = 1024 * 1024;
pub const MAX_TOKENS: usize = 16;
pub const CLIENT_VERSION: u32 = 6;

#[derive(Debug)]
pub enum Error {
    Der(der::Error),
    Invalid(&'static str),
    TooLarge,
    UnsupportedVersion(u32),
    ServerStatus(u32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Der(error) => write!(f, "invalid CredSSP DER: {error}"),
            Self::Invalid(reason) => write!(f, "invalid CredSSP message: {reason}"),
            Self::TooLarge => f.write_str("CredSSP message exceeds the local size limit"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "CredSSP version {version} is unsupported; version 5 or newer is required"
            ),
            Self::ServerStatus(status) => {
                write!(f, "CredSSP server returned NTSTATUS {status:#010x}")
            }
        }
    }
}

impl std::error::Error for Error {}
impl From<der::Error> for Error {
    fn from(error: der::Error) -> Self {
        Self::Der(error)
    }
}

/// Borrowed fields avoid copying authentication material while decoding.
#[derive(Clone, PartialEq, Eq)]
pub struct TsRequest<'a> {
    pub version: u32,
    pub nego_tokens: Vec<&'a [u8]>,
    /// Already sealed credentials; never supply plaintext credentials here.
    pub auth_info: Option<&'a [u8]>,
    /// Authentication-provider-protected TLS binding; not a raw public key.
    pub pub_key_auth: Option<&'a [u8]>,
    pub error_code: Option<u32>,
    pub client_nonce: Option<&'a [u8; 32]>,
}

impl fmt::Debug for TsRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TsRequest")
            .field("version", &self.version)
            .field("token_count", &self.nego_tokens.len())
            .field("has_auth_info", &self.auth_info.is_some())
            .field("has_pub_key_auth", &self.pub_key_auth.is_some())
            .field("error_code", &self.error_code)
            .field("has_client_nonce", &self.client_nonce.is_some())
            .finish()
    }
}

#[derive(Sequence)]
struct NegoToken<'a> {
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT")]
    token: OctetStringRef<'a>,
}

#[derive(Sequence)]
struct WireRequest<'a> {
    #[asn1(context_specific = "0", tag_mode = "EXPLICIT")]
    version: u32,
    #[asn1(context_specific = "1", tag_mode = "EXPLICIT", optional = "true")]
    nego_tokens: Option<SequenceOf<NegoToken<'a>, MAX_TOKENS>>,
    #[asn1(context_specific = "2", tag_mode = "EXPLICIT", optional = "true")]
    auth_info: Option<OctetStringRef<'a>>,
    #[asn1(context_specific = "3", tag_mode = "EXPLICIT", optional = "true")]
    pub_key_auth: Option<OctetStringRef<'a>>,
    #[asn1(context_specific = "4", tag_mode = "EXPLICIT", optional = "true")]
    error_code: Option<u32>,
    #[asn1(context_specific = "5", tag_mode = "EXPLICIT", optional = "true")]
    client_nonce: Option<OctetStringRef<'a>>,
}

impl<'a> TsRequest<'a> {
    pub fn new() -> Self {
        Self {
            version: CLIENT_VERSION,
            nego_tokens: Vec::new(),
            auth_info: None,
            pub_key_auth: None,
            error_code: None,
            client_nonce: None,
        }
    }

    /// This checks peer version/status only, not SPNEGO or TLS binding proofs.
    pub fn check_server_status(&self) -> Result<u32, Error> {
        // MS-CSSP 3.1.5 requires stopping whenever errorCode is present.
        if let Some(status) = self.error_code {
            return Err(Error::ServerStatus(status));
        }
        if self.version < 5 {
            return Err(Error::UnsupportedVersion(self.version));
        }
        Ok(self.version.min(CLIENT_VERSION))
    }

    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_MESSAGE_SIZE {
            return Err(Error::TooLarge);
        }
        // DER's context-specific helper permits extension skipping. TSRequest
        // has no extension marker: reject unknown, duplicate or unordered fields.
        let fields = SequenceOf::<AnyRef<'_>, 6>::from_der(bytes)?;
        let mut previous = None;
        for field in fields.iter() {
            let der::Tag::ContextSpecific {
                constructed: true,
                number,
            } = field.tag()
            else {
                return Err(Error::Invalid("expected explicit context-specific field"));
            };
            let number = number.value();
            if number > 5 || previous.is_some_and(|last| number <= last) {
                return Err(Error::Invalid("unknown, duplicate or unordered field"));
            }
            previous = Some(number);
        }
        let wire = WireRequest::from_der(bytes)?;
        if wire.version < 2 {
            return Err(Error::Invalid("version must be at least 2"));
        }
        let nonce = wire
            .client_nonce
            .map(|value| {
                value
                    .as_bytes()
                    .try_into()
                    .map_err(|_| Error::Invalid("client nonce must be 32 bytes"))
            })
            .transpose()?;
        if nonce.is_some() && wire.version < 5 {
            return Err(Error::Invalid("client nonce requires version 5 or newer"));
        }
        Ok(Self {
            version: wire.version,
            nego_tokens: wire
                .nego_tokens
                .map(|tokens| tokens.iter().map(|token| token.token.as_bytes()).collect())
                .unwrap_or_default(),
            auth_info: wire.auth_info.map(|value| value.as_bytes()),
            pub_key_auth: wire.pub_key_auth.map(|value| value.as_bytes()),
            error_code: wire.error_code,
            client_nonce: nonce,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.version < 2 {
            return Err(Error::Invalid("version must be at least 2"));
        }
        if self.client_nonce.is_some() && self.version < 5 {
            return Err(Error::Invalid("client nonce requires version 5 or newer"));
        }
        if self.nego_tokens.len() > MAX_TOKENS {
            return Err(Error::Invalid("too many negotiation tokens"));
        }
        let mut tokens = SequenceOf::new();
        for token in &self.nego_tokens {
            tokens.add(NegoToken {
                token: OctetStringRef::new(token)?,
            })?;
        }
        let wire = WireRequest {
            version: self.version,
            nego_tokens: if tokens.is_empty() {
                None
            } else {
                Some(tokens)
            },
            auth_info: self.auth_info.map(OctetStringRef::new).transpose()?,
            pub_key_auth: self.pub_key_auth.map(OctetStringRef::new).transpose()?,
            error_code: self.error_code,
            client_nonce: self
                .client_nonce
                .map(|value| OctetStringRef::new(value))
                .transpose()?,
        };
        if usize::try_from(wire.encoded_len()?)? > MAX_MESSAGE_SIZE {
            return Err(Error::TooLarge);
        }
        Ok(wire.to_der()?)
    }
}

impl Default for TsRequest<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Inspect a partial TLS plaintext buffer before allocating the whole message.
/// `None` means more header bytes are needed; `Some` is the total DER frame size.
/// Extra bytes can belong to the next frame and are not consumed here.
pub fn frame_length(prefix: &[u8]) -> Result<Option<usize>, Error> {
    let Some(&tag) = prefix.first() else {
        return Ok(None);
    };
    if tag != 0x30 {
        return Err(Error::Invalid("expected a sequence"));
    }
    let Some(&first) = prefix.get(1) else {
        return Ok(None);
    };
    let (header, content) = if first < 128 {
        (2, usize::from(first))
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 {
            return Err(Error::Invalid("indefinite lengths are not DER"));
        }
        if count > 3 {
            return Err(Error::TooLarge);
        }
        let Some(bytes) = prefix.get(2..2 + count) else {
            return Ok(None);
        };
        if bytes[0] == 0 {
            return Err(Error::Invalid("nonminimal DER length"));
        }
        let content = bytes
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        if content < 128 {
            return Err(Error::Invalid("nonminimal DER length"));
        }
        (2 + count, content)
    };
    let total = header + content;
    if total > MAX_MESSAGE_SIZE {
        return Err(Error::TooLarge);
    }
    Ok(Some(total))
}
