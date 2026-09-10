//! NTLMv2 provider adapter. SSPI performs token crypto and message sealing.
//! Raw NTLM tokens are permitted by MS-CSSP 2.2.1 (no SPNEGO wrapper here).

use linrdp_proto::{
    binding::{Protection, ProtectionError},
    credssp::MAX_MESSAGE_SIZE,
};
use sspi::{
    AuthIdentity, AuthIdentityBuffers, BufferType, ClientRequestFlags, CredentialUse,
    DataRepresentation, EncryptionFlags, Ntlm, SecurityBuffer, SecurityBufferRef, SecurityStatus,
    Sspi, SspiImpl,
};
use zeroize::Zeroizing;

type Error = Box<dyn std::error::Error>;

pub struct PendingNtlm {
    context: Ntlm,
    credentials: Option<AuthIdentityBuffers>,
    target: String,
}

impl PendingNtlm {
    /// Without identity this can emit only a credential-free Type 1 probe.
    pub fn begin(identity: Option<&AuthIdentity>, host: &str) -> Result<(Self, Vec<u8>), Error> {
        let mut context = Ntlm::new();
        let credentials = if let Some(identity) = identity {
            context
                .acquire_credentials_handle()
                .with_credential_use(CredentialUse::Outbound)
                .with_auth_data(identity)
                .execute(&mut context)
                .map_err(|_| "could not acquire NTLM credentials")?
                .credentials_handle
        } else {
            None
        };
        let mut pending = Self {
            context,
            credentials,
            target: format!("TERMSRV/{host}"),
        };
        let (status, token) = pending.step(&[])?;
        if status != SecurityStatus::ContinueNeeded {
            return Err("unexpected initial NTLM status".into());
        }
        Ok((pending, token))
    }

    pub fn finish(mut self, challenge: &[u8]) -> Result<(NtlmProtection, Vec<u8>), Error> {
        validate_challenge(challenge)?;
        if self.credentials.is_none() {
            return Err("NTLM authentication requires credentials".into());
        }
        let (status, token) = self.step(challenge)?;
        if status != SecurityStatus::Ok {
            return Err("NTLM authentication context did not complete".into());
        }
        Ok((
            NtlmProtection {
                context: self.context,
                failed: false,
            },
            token,
        ))
    }

    fn step(&mut self, input: &[u8]) -> Result<(SecurityStatus, Vec<u8>), Error> {
        let mut input = [SecurityBuffer::new(input.to_vec(), BufferType::Token)];
        let mut output = [SecurityBuffer::new(Vec::new(), BufferType::Token)];
        let mut builder = self
            .context
            .initialize_security_context()
            .with_credentials_handle(&mut self.credentials)
            .with_context_requirements(
                ClientRequestFlags::CONFIDENTIALITY
                    | ClientRequestFlags::INTEGRITY
                    | ClientRequestFlags::REPLAY_DETECT
                    | ClientRequestFlags::SEQUENCE_DETECT,
            )
            .with_target_data_representation(DataRepresentation::Native)
            .with_target_name(&self.target)
            .with_input(&mut input)
            .with_output(&mut output);
        let result = self
            .context
            .initialize_security_context_impl(&mut builder)
            .map_err(|_| "NTLM provider initialization failed")?
            .resolve_to_result()
            .map_err(|_| "NTLM token processing failed")?;
        Ok((result.status, std::mem::take(&mut output[0].buffer)))
    }
}

/// Check the minimum profile before asking the provider to authenticate.
pub fn validate_challenge(token: &[u8]) -> Result<(), Error> {
    if token.len() < 48
        || token.len() > MAX_MESSAGE_SIZE
        || &token[..8] != b"NTLMSSP\0"
        || token[8..12] != [2, 0, 0, 0]
    {
        return Err("expected an NTLM Type 2 challenge".into());
    }
    let flags = u32::from_le_bytes(token[20..24].try_into().unwrap());
    // MS-NLMP NegotiateFlags: UNICODE, SIGN, SEAL, NTLM, extended session
    // security, target info, 128-bit security and key exchange.
    const REQUIRED: u32 = 0x00000001
        | 0x00000010
        | 0x00000020
        | 0x00000200
        | 0x00080000
        | 0x00800000
        | 0x20000000
        | 0x40000000;
    if flags & REQUIRED != REQUIRED {
        return Err(
            "server NTLM challenge lacks required signing, sealing or NTLMv2 capabilities".into(),
        );
    }
    Ok(())
}

pub struct NtlmProtection {
    context: Ntlm,
    failed: bool,
}

impl Protection for NtlmProtection {
    fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ProtectionError> {
        if std::mem::replace(&mut self.failed, true) || plaintext.len() > MAX_MESSAGE_SIZE {
            return Err(ProtectionError);
        }
        let mut signature = [0; 16];
        let mut data = Zeroizing::new(plaintext.to_vec());
        let mut buffers = [
            SecurityBufferRef::token_buf(&mut signature),
            SecurityBufferRef::data_buf(&mut data),
        ];
        self.context
            .encrypt_message(EncryptionFlags::empty(), &mut buffers)
            .map_err(|_| ProtectionError)?;
        let mut output = buffers[0].data().to_vec();
        output.extend_from_slice(buffers[1].data());
        self.failed = false;
        Ok(output)
    }

    fn unseal(&mut self, protected: &[u8]) -> Result<Vec<u8>, ProtectionError> {
        if std::mem::replace(&mut self.failed, true)
            || protected.len() < 16
            || protected.len() > MAX_MESSAGE_SIZE
        {
            return Err(ProtectionError);
        }
        let mut signature = protected[..16].to_vec();
        let mut data = Zeroizing::new(protected[16..].to_vec());
        let mut buffers = [
            SecurityBufferRef::token_buf(&mut signature),
            SecurityBufferRef::data_buf(&mut data),
        ];
        self.context
            .decrypt_message(&mut buffers)
            .map_err(|_| ProtectionError)?;
        // Never return the provider's plaintext if integrity verification failed.
        let plaintext = buffers[1].data().to_vec();
        self.failed = false;
        Ok(plaintext)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sspi::{ServerRequestFlags, Username};

    pub(crate) fn identity(password: &str) -> AuthIdentity {
        AuthIdentity {
            username: Username::new("tester", Some("LAB")).unwrap(),
            password: password.to_owned().into(),
        }
    }

    pub(crate) struct TestServer {
        context: Ntlm,
        credentials: Option<AuthIdentityBuffers>,
    }
    impl TestServer {
        pub(crate) fn new(identity: &AuthIdentity) -> Self {
            let mut context = Ntlm::new();
            let credentials = context
                .acquire_credentials_handle()
                .with_credential_use(CredentialUse::Inbound)
                .with_auth_data(identity)
                .execute(&mut context)
                .unwrap()
                .credentials_handle;
            Self {
                context,
                credentials,
            }
        }
        pub(crate) fn accept(&mut self, bytes: &[u8]) -> Result<Vec<u8>, Error> {
            let mut input = [SecurityBuffer::new(bytes.to_vec(), BufferType::Token)];
            let mut output = [SecurityBuffer::new(Vec::new(), BufferType::Token)];
            let builder = self
                .context
                .accept_security_context()
                .with_credentials_handle(&mut self.credentials)
                .with_context_requirements(ServerRequestFlags::empty())
                .with_target_data_representation(DataRepresentation::Native)
                .with_input(&mut input)
                .with_output(&mut output);
            let result = self
                .context
                .accept_security_context_impl(builder)?
                .resolve_to_result()?;
            if matches!(
                result.status,
                SecurityStatus::CompleteNeeded | SecurityStatus::CompleteAndContinue
            ) {
                self.context.complete_auth_token(&mut output)?;
            }
            Ok(std::mem::take(&mut output[0].buffer))
        }
        pub(crate) fn protection(self) -> NtlmProtection {
            NtlmProtection {
                context: self.context,
                failed: false,
            }
        }
    }

    fn pair() -> (NtlmProtection, NtlmProtection) {
        let identity = identity("test password");
        let mut server = TestServer::new(&identity);
        let (pending, initial) = PendingNtlm::begin(Some(&identity), "rdp.example").unwrap();
        let challenge = server.accept(&initial).unwrap();
        let (client, final_token) = pending.finish(&challenge).unwrap();
        server.accept(&final_token).unwrap();
        (client, server.protection())
    }

    #[test]
    fn authenticates_and_seals_in_both_directions_with_sequence_numbers() {
        let (mut client, mut server) = pair();
        for message in [b"first".as_slice(), b"second", b"third"] {
            let protected = client.seal(message).unwrap();
            assert!(
                !protected
                    .windows(message.len())
                    .any(|window| window == message)
            );
            assert_eq!(server.unseal(&protected).unwrap(), message);
            let reply = server.seal(message).unwrap();
            assert_eq!(client.unseal(&reply).unwrap(), message);
        }
    }

    #[test]
    fn rejects_tampering_replay_and_reflection_and_poisoned_contexts() {
        let (mut client, mut server) = pair();
        let valid = client.seal(b"payload").unwrap();
        let mut tampered = valid.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(server.unseal(&tampered).is_err());
        assert!(server.unseal(&valid).is_err());
        let (mut client, mut server) = pair();
        let valid = client.seal(b"payload").unwrap();
        server.unseal(&valid).unwrap();
        assert!(server.unseal(&valid).is_err());
        assert!(client.unseal(&valid).is_err());
    }

    #[test]
    fn server_rejects_wrong_password() {
        let mut server = TestServer::new(&identity("correct"));
        let (pending, initial) = PendingNtlm::begin(Some(&identity("incorrect")), "host").unwrap();
        let challenge = server.accept(&initial).unwrap();
        let (_, final_token) = pending.finish(&challenge).unwrap();
        assert!(server.accept(&final_token).is_err());
    }

    #[test]
    fn probe_has_no_identity_and_cannot_authenticate() {
        let (pending, token) = PendingNtlm::begin(None, "host").unwrap();
        assert_eq!(&token[..12], b"NTLMSSP\0\x01\0\0\0");
        assert_eq!(&token[16..18], &[0, 0]); // domain length
        assert_eq!(&token[24..26], &[0, 0]); // workstation length
        let mut server = TestServer::new(&identity("password"));
        let challenge = server.accept(&token).unwrap();
        assert!(pending.finish(&challenge).is_err());
    }

    #[test]
    fn rejects_truncated_and_downgraded_challenges() {
        let (_, token) = PendingNtlm::begin(None, "host").unwrap();
        let mut server = TestServer::new(&identity("password"));
        let challenge = server.accept(&token).unwrap();
        for len in 0..48 {
            assert!(validate_challenge(&challenge[..len]).is_err());
        }
        for flag in [
            1u32, 0x10, 0x20, 0x200, 0x80000, 0x800000, 0x20000000, 0x40000000,
        ] {
            let mut downgraded = challenge.clone();
            let flags = u32::from_le_bytes(downgraded[20..24].try_into().unwrap()) & !flag;
            downgraded[20..24].copy_from_slice(&flags.to_le_bytes());
            assert!(validate_challenge(&downgraded).is_err());
        }
    }
}
