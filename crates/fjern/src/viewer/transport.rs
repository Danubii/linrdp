//! Ordered, bounded encrypted writes independent of the decoder/read thread.
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const BLOCK: usize = 16 * 1024;
const SLOTS: usize = 256;

pub(super) struct Transport {
    socket: TcpStream,
    output: Option<mpsc::SyncSender<Vec<u8>>>,
    completed: mpsc::Receiver<io::Result<()>>,
    writer: Option<JoinHandle<()>>,
    failed: bool,
    read_deadline: Option<Instant>,
}

impl Transport {
    pub(super) fn new(socket: TcpStream) -> io::Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(8)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut writer_socket = socket.try_clone()?;
        let (output, input) = mpsc::sync_channel::<Vec<u8>>(SLOTS);
        let (done, completed) = mpsc::channel();
        let writer = std::thread::Builder::new()
            .name("rdp-writer".into())
            .spawn(move || {
                let result = (|| {
                    for bytes in input {
                        // Absolute deadline, not a fresh five seconds per short write.
                        let deadline = Instant::now() + Duration::from_secs(5);
                        let mut rest = bytes.as_slice();
                        while !rest.is_empty() {
                            let left = deadline.saturating_duration_since(Instant::now());
                            if left.is_zero() {
                                return Err(io::ErrorKind::TimedOut.into());
                            }
                            writer_socket.set_write_timeout(Some(left))?;
                            match writer_socket.write(rest) {
                                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                                Ok(n) => rest = &rest[n..],
                                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                                Err(e) => return Err(e),
                            }
                        }
                    }
                    Ok(())
                })();
                if result.is_err() {
                    let _ = writer_socket.shutdown(Shutdown::Both);
                }
                let _ = done.send(result);
            })?;
        Ok(Self {
            socket,
            output: Some(output),
            completed,
            writer: Some(writer),
            failed: false,
            read_deadline: None,
        })
    }

    fn check(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("RDP writer stopped"));
        }
        match self.completed.try_recv() {
            Ok(result) => {
                self.failed = true;
                result.and(Err(io::Error::other("RDP writer stopped")))
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(io::Error::other("RDP writer disconnected"))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(()),
        }
    }

    pub(super) fn write_plaintext(
        &mut self,
        connection: &mut rustls::ClientConnection,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.check()?;
        while connection.wants_write() {
            connection.write_tls(self)?;
        }
        for chunk in bytes.chunks(16 * 1024) {
            connection.writer().write_all(chunk)?;
            while connection.wants_write() {
                connection.write_tls(self)?;
            }
        }
        Ok(())
    }

    pub(super) fn read_chunk(
        &mut self,
        connection: &mut rustls::ClientConnection,
        bytes: &mut [u8],
        budget: Duration,
    ) -> io::Result<usize> {
        self.check()?;
        self.read_deadline = Some(Instant::now() + budget);
        rustls::Stream::new(connection, self).read(bytes)
    }
}

impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        if let Some(deadline) = self.read_deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            self.socket.set_read_timeout(Some(left))?;
        }
        self.socket.read(bytes)
    }
}
impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let n = bytes.len().min(BLOCK);
        self.output
            .as_ref()
            .unwrap()
            .try_send(bytes[..n].to_vec())
            .map_err(|_| io::Error::other("RDP encrypted output queue unavailable"))?;
        Ok(n)
    }
    // Bytes accepted here remain ordered in the bounded writer queue. This
    // is not an acknowledgement from the peer or permission to drop bytes.
    fn flush(&mut self) -> io::Result<()> {
        self.check()
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.output.take();
        // Brief graceful drain (including key releases), then cancel stalled IO.
        let _ = self.completed.recv_timeout(Duration::from_millis(100));
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tls_records_round_trip_beyond_internal_plaintext_buffer_limit() {
        use std::sync::Arc;
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let server_config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
                )
                .unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            peer.set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut connection = rustls::ServerConnection::new(server_config).unwrap();
            let mut tls = rustls::Stream::new(&mut connection, &mut peer);
            let mut received = vec![0; 200_000];
            tls.read_exact(&mut received).unwrap();
            assert!(received.iter().enumerate().all(|(i, b)| *b == i as u8));
            tls.write_all(b"frame-ready").unwrap();
            tls.flush().unwrap();
        });
        let mut connection = crate::tls::handshake(
            &mut socket,
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
            client_config,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        let mut transport = Transport::new(socket).unwrap();
        let payload: Vec<_> = (0..200_000).map(|n| n as u8).collect();
        transport
            .write_plaintext(&mut connection, &payload)
            .unwrap();
        let mut reply = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while reply.len() < 11 {
            let mut bytes = [0; 11];
            match transport.read_chunk(&mut connection, &mut bytes, Duration::from_millis(8)) {
                Ok(0) => panic!("early EOF"),
                Ok(n) => reply.extend_from_slice(&bytes[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => panic!("{e}"),
            }
            assert!(Instant::now() < deadline);
        }
        assert_eq!(reply, b"frame-ready");
        server.join().unwrap();
    }
    fn pair() -> (Transport, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        (
            Transport::new(socket).unwrap(),
            listener.accept().unwrap().0,
        )
    }
    #[test]
    fn encrypted_writes_are_ordered_and_shutdown_drains() {
        let (mut transport, mut peer) = pair();
        transport.write_all(b"key-down").unwrap();
        transport.write_all(b"key-up").unwrap();
        drop(transport);
        let mut bytes = Vec::new();
        peer.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"key-downkey-up");
    }
    #[test]
    fn stalled_output_keeps_reads_live_and_drop_is_bounded() {
        let (mut transport, mut peer) = pair();
        // Peer deliberately never drains its receive window.
        let block = vec![42; BLOCK];
        let started = Instant::now();
        loop {
            if transport.write(&block).is_err() {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        peer.write_all(b"frame").unwrap();
        let mut incoming = [0; 5];
        transport.read_exact(&mut incoming).unwrap();
        assert_eq!(&incoming, b"frame");
        let started = Instant::now();
        drop(transport);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
