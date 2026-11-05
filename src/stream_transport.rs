//! Verified TLS for both SHOUTcast source and metadata connections.
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionSecurity {
    #[default]
    Tls,
    Plaintext,
}

impl ConnectionSecurity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tls => "TLS (certificate verified)",
            Self::Plaintext => "Unencrypted legacy TCP / HTTP",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Connector {
    config: Arc<ClientConfig>,
}

impl Default for Connector {
    fn default() -> Self {
        static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
        Self {
            config: CONFIG
                .get_or_init(|| {
                    let roots =
                        RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                    Arc::new(
                        ClientConfig::builder()
                            .with_root_certificates(roots)
                            .with_no_client_auth(),
                    )
                })
                .clone(),
        }
    }
}

impl Connector {
    #[cfg(test)]
    pub(crate) fn with_config(config: Arc<ClientConfig>) -> Self {
        Self { config }
    }

    pub(crate) fn connect(
        &self,
        host: &str,
        port: u16,
        security: ConnectionSecurity,
        stop: &AtomicBool,
    ) -> Result<Connection, String> {
        let host = host.trim().trim_start_matches('[').trim_end_matches(']');
        let mut socket = connect_tcp(host, port, stop)?;
        if security == ConnectionSecurity::Plaintext {
            return Ok(Connection::Plaintext(socket));
        }
        let server_name = ServerName::try_from(host.to_owned())
            .map_err(|_| "Invalid TLS server hostname or IP address.".to_owned())?;
        let mut client = ClientConnection::new(self.config.clone(), server_name)
            .map_err(|e| format!("Could not initialize TLS: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        // No caller can write credentials until the handshake and certificate /
        // hostname verification have completed. Never reconnect in plaintext.
        while client.is_handshaking() || client.wants_write() {
            if stop.load(Ordering::Acquire) {
                return Err("Connection cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err("TLS handshake timed out. No credentials were sent.".into());
            }
            // Check cancellation/deadline between individual I/O operations,
            // including when a peer sends a TLS record in very small pieces.
            let result = if client.wants_write() {
                client.write_tls(&mut socket)
            } else {
                client.read_tls(&mut socket)
            };
            match result {
                Ok(0) => return Err("TLS peer closed the handshake. No credentials were sent; no plaintext fallback.".into()),
                Ok(_) => {
                    client.process_new_packets().map_err(|e| format!("TLS verification/handshake failed: {e}. No credentials were sent; no plaintext fallback."))?;
                }
                Err(e) if retryable(&e) => continue,
                Err(e) => {
                    return Err(format!(
                        "TLS verification/handshake failed: {e}. No credentials were sent; no plaintext fallback."
                    ));
                }
            }
        }
        // Nonblocking TLS I/O lets the caller bound complete-record reads and
        // writes, rather than resetting a timeout for every partial TCP read.
        socket
            .set_nonblocking(true)
            .map_err(|e| format!("Could not configure TLS socket: {e}"))?;
        Ok(Connection::Tls(Box::new(StreamOwned::new(client, socket))))
    }
}

pub(crate) enum Connection {
    Plaintext(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Connection {
    pub(crate) fn shutdown(&mut self) -> io::Result<()> {
        match self {
            Self::Plaintext(socket) => socket.shutdown(Shutdown::Both),
            Self::Tls(stream) => {
                stream.conn.send_close_notify();
                let _ = stream.flush();
                stream.sock.shutdown(Shutdown::Both)
            }
        }
    }
}

impl Read for Connection {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plaintext(socket) => socket.read(bytes),
            Self::Tls(stream) => stream.read(bytes),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plaintext(socket) => socket.write(bytes),
            Self::Tls(stream) => stream.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plaintext(socket) => socket.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

pub(crate) fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn connect_tcp(host: &str, port: u16, stop: &AtomicBool) -> Result<TcpStream, String> {
    let host = host.to_owned();
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(
            (host.as_str(), port)
                .to_socket_addrs()
                .map(|a| a.collect::<Vec<_>>()),
        );
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let addresses = loop {
        if stop.load(Ordering::Acquire) {
            return Err("Connection cancelled".into());
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(addresses)) => break addresses,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Could not resolve server hostname.".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                return Err("Server lookup timed out.".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    // Try every resolved address before retrying a timeout. A black-holed IPv6
    // address must not consume the entire deadline and hide a working IPv4 peer.
    let mut pending = addresses;
    while !pending.is_empty() && Instant::now() < deadline {
        let mut timed_out = Vec::new();
        for address in pending {
            if Instant::now() >= deadline {
                break;
            }
            if stop.load(Ordering::Acquire) {
                return Err("Connection cancelled".into());
            }
            match TcpStream::connect_timeout(&address, Duration::from_millis(250)) {
                Ok(socket) => {
                    socket
                        .set_read_timeout(Some(Duration::from_millis(200)))
                        .map_err(|e| e.to_string())?;
                    socket
                        .set_write_timeout(Some(Duration::from_millis(200)))
                        .map_err(|e| e.to_string())?;
                    let _ = socket.set_nodelay(true);
                    return Ok(socket);
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => timed_out.push(address),
                Err(_) => {}
            }
        }
        pending = timed_out;
    }
    Err("SHOUTcast server/port is unreachable.".into())
}
