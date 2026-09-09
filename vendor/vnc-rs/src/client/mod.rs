mod auth;
pub mod connection;
pub mod connector;
mod messages;
mod security;

pub use connection::VncClient;
pub use connector::VncConnector;

/// Compute the classic VNC DES challenge response for a password.
pub fn password_response(challenge: &[u8; 16], password: &str) -> Vec<u8> {
    let mut key = zeroize::Zeroizing::new([0u8; 8]);
    for (out, byte) in key.iter_mut().zip(password.as_bytes()) {
        *out = byte.reverse_bits();
    }
    security::des::encrypt(challenge, &key)
}
