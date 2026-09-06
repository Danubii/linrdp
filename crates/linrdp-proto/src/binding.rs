//! CredSSP v5/v6 TLS binding stage (MS-CSSP 3.1.5).
//! The caller must first verify TLS and complete the authentication provider's
//! token exchange. This module does not implement NTLM, Kerberos or SPNEGO.

use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use subtle::ConstantTimeEq;

use crate::credssp::{CLIENT_VERSION, Error, MAX_MESSAGE_SIZE, TsRequest};

/// A completed authentication context, including integrity, confidentiality,
/// directional keys and sequence-number enforcement. Implementations must
/// reject bad signatures before returning plaintext from `unseal`.
pub trait Protection {
    fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ProtectionError>;
    fn unseal(&mut self, protected: &[u8]) -> Result<Vec<u8>, ProtectionError>;
}

/// Deliberately carries no provider payloads or credentials into error logs.
#[derive(Debug)]
pub struct ProtectionError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Ready,
    AwaitingServer,
    Verified,
    Delegated,
    Failed,
}

/// Owns a completed provider context. No Debug implementation exposes its keys.
/// Supply the SubjectPublicKey BIT STRING contents from the verified TLS leaf
/// certificate, without the BIT STRING wrapper or unused-bits byte.
pub struct BindingClient<P> {
    protection: P,
    nonce: [u8; 32],
    client_hash: [u8; 32],
    server_hash: [u8; 32],
    version: u32,
    state: State,
}

impl<P: Protection> BindingClient<P> {
    pub fn new(protection: P, public_key: &[u8], peer_version: u32) -> Result<Self, Error> {
        let mut nonce = [0; 32];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| Error::Invalid("secure nonce generation failed"))?;
        Self::with_nonce(protection, public_key, peer_version, nonce)
    }

    fn with_nonce(
        protection: P,
        public_key: &[u8],
        peer_version: u32,
        nonce: [u8; 32],
    ) -> Result<Self, Error> {
        if peer_version < 5 {
            return Err(Error::UnsupportedVersion(peer_version));
        }
        if public_key.is_empty() || public_key.len() > MAX_MESSAGE_SIZE {
            return Err(Error::Invalid("invalid TLS public key length"));
        }
        Ok(Self {
            protection,
            nonce,
            client_hash: hash(
                b"CredSSP Client-To-Server Binding Hash\0",
                &nonce,
                public_key,
            ),
            server_hash: hash(
                b"CredSSP Server-To-Client Binding Hash\0",
                &nonce,
                public_key,
            ),
            version: peer_version.min(CLIENT_VERSION),
            state: State::Ready,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Send the final authentication token together with the sealed binding.
    /// No credentials are sent in this message.
    pub fn request(&mut self, final_token: &[u8]) -> Result<Vec<u8>, Error> {
        let previous = std::mem::replace(&mut self.state, State::Failed);
        if previous != State::Ready || final_token.is_empty() {
            return Err(Error::Invalid(
                "binding request is out of sequence or missing its final token",
            ));
        }
        if final_token.len() > MAX_MESSAGE_SIZE {
            return Err(Error::TooLarge);
        }
        let sealed = self
            .protection
            .seal(&self.client_hash)
            .map_err(|_| Error::Invalid("binding protection failed"))?;
        let mut request = TsRequest::new();
        request.version = self.version;
        request.nego_tokens.push(final_token);
        request.pub_key_auth = Some(&sealed);
        request.client_nonce = Some(&self.nonce);
        let encoded = request.encode()?;
        self.state = State::AwaitingServer;
        Ok(encoded)
    }

    /// An error permanently fails this exchange, including provider failures,
    /// reflected client proofs and changed negotiated versions.
    pub fn verify(&mut self, message: &[u8]) -> Result<(), Error> {
        let previous = std::mem::replace(&mut self.state, State::Failed);
        if previous != State::AwaitingServer {
            return Err(Error::Invalid("unexpected server binding response"));
        }
        let response = TsRequest::decode(message)?;
        if response.check_server_status()? != self.version {
            return Err(Error::Invalid("CredSSP version changed during binding"));
        }
        if !response.nego_tokens.is_empty()
            || response.auth_info.is_some()
            || response.client_nonce.is_some()
        {
            return Err(Error::Invalid(
                "unexpected fields in server binding response",
            ));
        }
        let sealed = response
            .pub_key_auth
            .ok_or(Error::Invalid("missing server binding proof"))?;
        let proof = self
            .protection
            .unseal(sealed)
            .map_err(|_| Error::Invalid("server binding protection failed"))?;
        if proof.len() != 32 || !bool::from(proof.as_slice().ct_eq(&self.server_hash)) {
            return Err(Error::Invalid("server TLS binding does not match"));
        }
        self.state = State::Verified;
        Ok(())
    }

    /// Seal an already encoded TSCredentials only after verified server binding.
    /// The caller owns/zeroizes plaintext storage. Success means the message is
    /// ready to send, not that Windows accepted the login or started a desktop.
    pub fn delegate(&mut self, credentials: &[u8]) -> Result<Vec<u8>, Error> {
        let previous = std::mem::replace(&mut self.state, State::Failed);
        if previous != State::Verified {
            return Err(Error::Invalid(
                "credentials require verified server binding",
            ));
        }
        if credentials.is_empty() || credentials.len() > MAX_MESSAGE_SIZE {
            return Err(Error::Invalid("invalid credential payload length"));
        }
        let sealed = self
            .protection
            .seal(credentials)
            .map_err(|_| Error::Invalid("credential protection failed"))?;
        let mut request = TsRequest::new();
        request.version = self.version;
        request.auth_info = Some(&sealed);
        let encoded = request.encode()?;
        self.state = State::Delegated;
        Ok(encoded)
    }
}

fn hash(label: &[u8], nonce: &[u8; 32], public_key: &[u8]) -> [u8; 32] {
    let mut context = digest::Context::new(&digest::SHA256);
    context.update(label);
    context.update(nonce);
    context.update(public_key);
    context
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA256 is 32 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    // Scripted provider only: deliberately not cryptography or wire-compatible
    // NTLM. It verifies which plaintext reaches the provider, and when.
    struct Fake {
        proof: Vec<u8>,
        writes: Rc<RefCell<Vec<Vec<u8>>>>,
        fail: bool,
    }
    impl Protection for Fake {
        fn seal(&mut self, bytes: &[u8]) -> Result<Vec<u8>, ProtectionError> {
            if self.fail {
                return Err(ProtectionError);
            }
            self.writes.borrow_mut().push(bytes.to_vec());
            Ok(b"protected-output".to_vec())
        }
        fn unseal(&mut self, _: &[u8]) -> Result<Vec<u8>, ProtectionError> {
            if self.fail {
                return Err(ProtectionError);
            }
            Ok(self.proof.clone())
        }
    }
    fn fixture() -> BindingClient<Fake> {
        let nonce = std::array::from_fn(|i| i as u8);
        let proof = hash(
            b"CredSSP Server-To-Client Binding Hash\0",
            &nonce,
            b"public-key-fixture",
        );
        BindingClient::with_nonce(
            Fake {
                proof: proof.to_vec(),
                writes: Rc::default(),
                fail: false,
            },
            b"public-key-fixture",
            6,
            nonce,
        )
        .unwrap()
    }
    fn response() -> Vec<u8> {
        let mut request = TsRequest::new();
        request.pub_key_auth = Some(b"provider-protected-proof");
        request.encode().unwrap()
    }

    #[test]
    fn matches_independent_python_hashlib_vectors() {
        let client = fixture();
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(&client.client_hash),
            "14592d7a32c84ecf60da044b3f92871d3f70fc6e0b9125eb7014873b86c74a45"
        );
        assert_eq!(
            hex(&client.server_hash),
            "b8f44de66a7aed29e0d25169bf95440ea3748b3d815eb1371ed7d14c6996b7fc"
        );
    }

    #[test]
    fn only_delegates_after_verifying_directional_binding() {
        let mut client = fixture();
        let bytes = client.request(b"final-token").unwrap();
        let request = TsRequest::decode(&bytes).unwrap();
        assert!(request.auth_info.is_none());
        assert_eq!(request.client_nonce, Some(&client.nonce));
        assert_eq!(request.nego_tokens, vec![b"final-token".as_slice()]);
        assert_eq!(client.state(), State::AwaitingServer);
        client.verify(&response()).unwrap();
        assert_eq!(client.state(), State::Verified);
        let bytes = client.delegate(b"encoded credentials").unwrap();
        let request = TsRequest::decode(&bytes).unwrap();
        assert!(request.nego_tokens.is_empty());
        assert!(request.pub_key_auth.is_none());
        assert!(request.client_nonce.is_none());
        assert_eq!(request.auth_info, Some(b"protected-output".as_slice()));
        assert_eq!(client.state(), State::Delegated);
        assert_eq!(client.protection.writes.borrow()[1], b"encoded credentials");
        assert!(client.delegate(b"again").is_err());
    }

    #[test]
    fn rejects_early_delegation_without_calling_provider() {
        for send_request in [false, true] {
            let mut client = fixture();
            if send_request {
                client.request(b"token").unwrap();
            }
            let before = client.protection.writes.borrow().len();
            assert!(client.delegate(b"credentials").is_err());
            assert_eq!(client.protection.writes.borrow().len(), before);
            assert_eq!(client.state(), State::Failed);
            assert!(client.verify(&response()).is_err());
        }
    }

    #[test]
    fn rejects_reflection_wrong_key_nonce_and_proof_length() {
        let reference = fixture();
        for proof in [
            reference.client_hash.to_vec(),
            vec![0; 31],
            vec![0; 33],
            hash(
                b"CredSSP Server-To-Client Binding Hash\0",
                &[99; 32],
                b"public-key-fixture",
            )
            .to_vec(),
            hash(
                b"CredSSP Server-To-Client Binding Hash\0",
                &reference.nonce,
                b"other-key",
            )
            .to_vec(),
        ] {
            let mut client = fixture();
            client.protection.proof = proof;
            client.request(b"token").unwrap();
            assert!(client.verify(&response()).is_err());
            assert_eq!(client.state(), State::Failed);
            assert!(client.delegate(b"credentials").is_err());
        }
    }

    #[test]
    fn rejects_error_status_version_changes_and_unexpected_fields() {
        for case in 0..6 {
            let mut client = fixture();
            client.request(b"token").unwrap();
            let mut request = TsRequest::new();
            request.pub_key_auth = Some(b"proof");
            match case {
                0 => request.error_code = Some(0xc000006d),
                1 => request.version = 5,
                2 => request.auth_info = Some(b"unexpected"),
                3 => request.nego_tokens.push(b"unexpected"),
                4 => request.client_nonce = Some(&[0; 32]),
                _ => request.pub_key_auth = None,
            }
            assert!(client.verify(&request.encode().unwrap()).is_err());
            assert_eq!(client.state(), State::Failed);
        }
    }

    #[test]
    fn provider_errors_and_invalid_sequences_are_terminal() {
        let mut client = fixture();
        assert!(client.verify(&response()).is_err());
        assert!(client.request(b"token").is_err());
        let mut client = fixture();
        client.protection.fail = true;
        assert!(client.request(b"token").is_err());
        assert_eq!(client.state(), State::Failed);
        let mut client = fixture();
        client.request(b"token").unwrap();
        client.protection.fail = true;
        assert!(client.verify(&response()).is_err());
        assert_eq!(client.state(), State::Failed);
    }

    #[test]
    fn generates_fresh_nonces() {
        let first = BindingClient::new(fixture().protection, b"key", 6).unwrap();
        let second = BindingClient::new(fixture().protection, b"key", 6).unwrap();
        assert_ne!(first.nonce, second.nonce);
    }
}
