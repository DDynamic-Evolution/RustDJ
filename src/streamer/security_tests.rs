use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, date_time_ymd,
};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
    pki_types::PrivatePkcs8KeyDer,
};
use std::{
    io::{BufRead, BufReader},
    net::{Shutdown, TcpListener, TcpStream},
};

struct WireTap {
    socket: TcpStream,
    received: Arc<Mutex<Vec<u8>>>,
}
impl Read for WireTap {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.socket.read(bytes)?;
        self.received
            .lock()
            .unwrap()
            .extend_from_slice(&bytes[..count]);
        Ok(count)
    }
}
impl Write for WireTap {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}
type ServerStream = StreamOwned<ServerConnection, WireTap>;
type Observed = Arc<Mutex<Vec<u8>>>;

fn certificates(
    expired: bool,
    version: &'static rustls::SupportedProtocolVersion,
) -> (Arc<ServerConfig>, Connector) {
    let key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca = ca_params.self_signed(&key).unwrap();
    let issuer = Issuer::from_params(&ca_params, &key);
    let server_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    if expired {
        params.not_before = date_time_ymd(2000, 1, 1);
        params.not_after = date_time_ymd(2001, 1, 1);
    }
    let certificate = params.signed_by(&server_key, &issuer).unwrap();
    let server = ServerConfig::builder_with_protocol_versions(&[version])
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone(), ca.der().clone()],
            PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )
        .unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(ca.der().clone()).unwrap();
    let client = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (Arc::new(server), Connector::with_config(Arc::new(client)))
}

fn tls_server(
    config: Arc<ServerConfig>,
    handler: impl FnOnce(&mut ServerStream) + Send + 'static,
) -> (u16, thread::JoinHandle<()>, Observed) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let observed = received.clone();
    let handle = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut stream = StreamOwned::new(
            ServerConnection::new(config).unwrap(),
            WireTap { socket, received },
        );
        handler(&mut stream);
        stream.conn.send_close_notify();
        let _ = stream.flush();
        let _ = stream.sock.socket.shutdown(Shutdown::Both);
        // A failed TLS connection must not be followed by a plaintext retry.
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock));
    });
    (port, handle, observed)
}

fn no_secrets(wire: &Observed, password: &str) {
    let wire = wire.lock().unwrap();
    assert!(!wire.is_empty());
    for marker in [
        password.as_bytes(),
        percent(password).as_bytes(),
        b"GET /admin.cgi",
        b"icy-name:",
        b"test-audio-frame",
    ] {
        assert!(
            !wire.windows(marker.len()).any(|part| part == marker),
            "plaintext on TLS wire"
        );
    }
}

#[test]
fn tls20_source_and_metadata_are_encrypted_and_verified() {
    let password = "local-TLS-only-Pass&?";
    let title = "Björk & Test ? / Live";
    let stop = AtomicBool::new(false);
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let (server, connector) = certificates(false, version);
        let payload = b"test-audio-frame".repeat(16000);
        let expected = payload.clone();
        let (source_port, source, source_wire) = tls_server(server.clone(), move |stream| {
            let mut reader = BufReader::new(stream);
            let mut auth = String::new();
            reader.read_line(&mut auth).unwrap();
            assert_eq!(auth, format!("{password}:#2\r\n"));
            reader.get_mut().write_all(b"OK2\r\n").unwrap();
            reader.get_mut().flush().unwrap();
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
            }
            assert!(headers.contains("content-type:audio/mpeg"));
            let mut audio = Vec::new();
            reader.read_to_end(&mut audio).unwrap();
            assert_eq!(audio, expected);
        });
        let (admin_port, admin, admin_wire) = tls_server(server, move |stream| {
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            assert!(request.contains("GET /admin.cgi?sid=2&mode=updinfo"));
            assert!(request.contains(&format!("pass={}", percent(password))));
            assert!(request.contains(&format!("song={}", percent(title))));
            reader
                .get_mut()
                .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                .unwrap();
            reader.get_mut().flush().unwrap();
        });
        let settings = StreamSettings {
            host: "localhost".into(),
            source_port,
            admin_port,
            stream_id: 2,
            ..Default::default()
        };
        let mut connection = open_source(&connector, &settings, password, &stop).unwrap();
        match &connection {
            Connection::Tls(stream) => {
                assert_eq!(stream.conn.protocol_version(), Some(version.version))
            }
            Connection::Plaintext(_) => panic!("TLS is required by default"),
        }
        let mut output = NetworkWriter {
            socket: &mut connection,
            stop: &stop,
            stop_deadline: None,
        };
        output.write_all(&payload).unwrap();
        output.flush().unwrap();
        connection.shutdown().unwrap();
        send_metadata(&connector, &settings, password, title, &stop).unwrap();
        source.join().unwrap();
        admin.join().unwrap();
        no_secrets(&source_wire, password);
        no_secrets(&admin_wire, password);
    }
}

#[test]
fn tls20_rejects_bad_certificates_plaintext_peers_and_downgrades() {
    let password = "must-never-appear-on-failed-TLS";
    let stop = AtomicBool::new(false);
    let legacy: StreamSettings = serde_json::from_str(r#"{"host":"saved.example.invalid","source_port":9021,"admin_port":9020,"protected_password":[1,2,3]}"#).unwrap();
    assert_eq!(legacy.security, ConnectionSecurity::Tls);
    assert_eq!(legacy.source_port, 9021);
    assert_eq!(legacy.admin_port, 9020);
    assert_eq!(legacy.protected_password, [1, 2, 3]);
    let restored: StreamSettings =
        serde_json::from_str(&serde_json::to_string(&legacy).unwrap()).unwrap();
    assert_eq!(restored, legacy);
    for failure in ["untrusted", "wrong-host", "expired"] {
        let (server, trusted) = certificates(failure == "expired", &rustls::version::TLS13);
        let connector = if failure == "untrusted" {
            Connector::default()
        } else {
            trusted
        };
        for metadata in [false, true] {
            let (port, handle, wire) = tls_server(server.clone(), |stream| {
                let mut plaintext = Vec::new();
                let _ = stream.read_to_end(&mut plaintext);
                assert!(
                    plaintext.is_empty(),
                    "application data sent before certificate verification"
                );
            });
            let settings = StreamSettings {
                host: if failure == "wrong-host" {
                    "127.0.0.1"
                } else {
                    "localhost"
                }
                .into(),
                source_port: port,
                admin_port: port,
                ..Default::default()
            };
            let error = if metadata {
                send_metadata(&connector, &settings, password, "test title", &stop).unwrap_err()
            } else {
                match open_source(&connector, &settings, password, &stop) {
                    Err(error) => error,
                    Ok(_) => panic!("invalid server certificate accepted"),
                }
            };
            assert!(error.contains("TLS"), "{failure}: {error}");
            assert!(!error.contains(password));
            handle.join().unwrap();
            no_secrets(&wire, password);
        }
    }
    // A real legacy TCP/HTTP peer must never receive an automatic downgrade.
    for metadata in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut received = vec![0; 4096];
            let count = socket.read(&mut received).unwrap();
            received.truncate(count);
            let _ = socket.write_all(b"OK2\r\nHTTP/1.0 200 OK\r\n\r\n");
            let _ = socket.read_to_end(&mut received);
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock));
            received
        });
        let settings = StreamSettings {
            host: "127.0.0.1".into(),
            source_port: port,
            admin_port: port,
            ..Default::default()
        };
        let connector = Connector::default();
        let failed = if metadata {
            send_metadata(&connector, &settings, password, "test", &stop).is_err()
        } else {
            open_source(&connector, &settings, password, &stop).is_err()
        };
        assert!(failed);
        let wire = Arc::new(Mutex::new(server.join().unwrap()));
        no_secrets(&wire, password);
    }
    // Cancellation interrupts a stalled handshake without sending credentials.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = stopped.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut initial = [0; 4096];
        assert!(socket.read(&mut initial).unwrap() > 0);
        signal.store(true, Ordering::Release);
        let mut remaining = Vec::new();
        let _ = socket.read_to_end(&mut remaining);
        assert!(
            !remaining
                .windows(password.len())
                .any(|part| part == password.as_bytes())
        );
    });
    let start = Instant::now();
    let result = Connector::default().connect("localhost", port, ConnectionSecurity::Tls, &stopped);
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    server.join().unwrap();
    // Plain TCP is available only after an explicit, serializable mode change.
    let mut settings = legacy;
    settings.security = ConnectionSecurity::Plaintext;
    let json = serde_json::to_string(&settings).unwrap();
    assert!(json.contains(r#""security":"plaintext""#));
    assert_eq!(
        serde_json::from_str::<StreamSettings>(&json)
            .unwrap()
            .security,
        ConnectionSecurity::Plaintext
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    settings.host = "127.0.0.1".into();
    settings.source_port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut auth = String::new();
        reader.read_line(&mut auth).unwrap();
        assert_eq!(auth, format!("{password}\r\n"));
        socket.write_all(b"OK2\r\n").unwrap();
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
    });
    let mut plain = open_source(&Connector::default(), &settings, password, &stop).unwrap();
    plain.shutdown().unwrap();
    server.join().unwrap();
}
