//! SHOUTcast source connection over verified TLS (including DNAS 2 stream IDs),
//! optional song updates, and independent MP3 master recordings.
use crate::capture::{MasterCapture, PcmBuffer};
pub use crate::stream_transport::ConnectionSecurity;
use crate::stream_transport::{Connection, Connector, retryable};
use mp3lame_encoder::{Bitrate, Builder, Encoder, FlushGap, InterleavedPcm, Quality, VbrMode};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamSettings {
    pub protected_password: Vec<u8>,
    pub host: String,
    pub source_port: u16,
    pub admin_port: u16,
    pub security: ConnectionSecurity,
    pub stream_id: u32,
    pub station: String,
    pub genre: String,
    pub url: String,
    pub bitrate: u16,
    pub send_tags: bool,
    pub automatic_title: bool,
    pub manual_title: String,
    pub recording_dir: PathBuf,
}

impl Default for StreamSettings {
    fn default() -> Self {
        let base = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            protected_password: Vec::new(),
            host: String::new(),
            source_port: 8001,
            admin_port: 8000,
            security: ConnectionSecurity::Tls,
            stream_id: 1,
            station: "RustDJ".into(),
            genre: String::new(),
            url: String::new(),
            bitrate: 192,
            send_tags: false,
            automatic_title: true,
            manual_title: String::new(),
            recording_dir: base.join("Music").join("RustDJ-Recordings"),
        }
    }
}

impl StreamSettings {
    fn validate(&self, password: &str) -> Result<(), String> {
        if self.host.trim().is_empty() || self.host.contains(['/', '\\', '\r', '\n', ' ', '\t']) {
            return Err(
                "Enter the server as a hostname or IP address without http:// or a path.".into(),
            );
        }
        if self.source_port == 0 || self.admin_port == 0 || self.stream_id == 0 {
            return Err("Ports and stream ID must be greater than zero.".into());
        }
        if password.is_empty() || password.chars().any(char::is_control) {
            return Err("Enter a valid source password.".into());
        }
        if [&self.station, &self.genre, &self.url]
            .iter()
            .any(|s| s.chars().any(char::is_control))
        {
            return Err("Station details cannot contain line breaks or control characters.".into());
        }
        bitrate(self.bitrate)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Running,
    Stopping,
    Done,
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub phase: Phase,
    pub message: String,
    pub bytes: u64,
    pub seconds: f64,
    pub path: Option<PathBuf>,
    pub metadata_error: Option<String>,
}

impl Status {
    pub fn label(&self) -> &str {
        match self.phase {
            Phase::Idle => "OFF",
            Phase::Starting => "STARTING …",
            Phase::Running => "ACTIVE",
            Phase::Stopping => "STOPPING …",
            Phase::Done => "STOPPED",
            Phase::Error => "ERROR",
        }
    }
}

type SharedStatus = Arc<Mutex<Status>>;
fn update(status: &SharedStatus, change: impl FnOnce(&mut Status)) {
    change(&mut status.lock().unwrap_or_else(|e| e.into_inner()));
}

struct Job {
    stop: Arc<AtomicBool>,
    status: SharedStatus,
    thread: JoinHandle<()>,
}

impl Job {
    fn spawn(
        work: impl FnOnce(Arc<AtomicBool>, SharedStatus) -> Result<(), String> + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status {
            phase: Phase::Starting,
            ..Status::default()
        }));
        let thread_stop = stop.clone();
        let thread_status = status.clone();
        let thread = thread::spawn(move || {
            let result = work(thread_stop.clone(), thread_status.clone());
            update(&thread_status, |status| match result {
                Ok(()) => {
                    status.phase = Phase::Done;
                    status.message = "Stopped".into();
                }
                Err(error) if thread_stop.load(Ordering::Acquire) => {
                    status.phase = Phase::Done;
                    status.message = format!("Stopped: {error}");
                }
                Err(error) => {
                    status.phase = Phase::Error;
                    status.message = error;
                }
            });
        });
        Self {
            stop,
            status,
            thread,
        }
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        update(&self.status, |status| {
            if matches!(status.phase, Phase::Starting | Phase::Running) {
                status.phase = Phase::Stopping;
            }
        });
    }
}

#[derive(Default)]
struct Metadata {
    enabled: bool,
    title: String,
}

#[derive(Default)]
pub struct Broadcaster {
    pub stream_status: Status,
    pub recording_status: Status,
    stream_job: Option<Job>,
    recording_job: Option<Job>,
    metadata: Arc<Mutex<Metadata>>,
}

impl Broadcaster {
    pub fn stream_busy(&self) -> bool {
        self.stream_job.is_some()
    }
    pub fn recording_busy(&self) -> bool {
        self.recording_job.is_some()
    }
    pub fn busy(&self) -> bool {
        self.stream_busy() || self.recording_busy()
    }

    pub fn start_stream(
        &mut self,
        capture: Arc<MasterCapture>,
        settings: StreamSettings,
        password: String,
    ) -> Result<(), String> {
        if self.stream_busy() {
            return Err("The stream is already running or is still stopping.".into());
        }
        settings.validate(&password)?;
        self.metadata
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .enabled = settings.send_tags;
        let metadata = self.metadata.clone();
        self.stream_job = Some(Job::spawn(move |stop, status| {
            let connector = Connector::default();
            let mut socket = open_source(&connector, &settings, &password, &stop)?;
            let encoder = encoder(capture.sample_rate, settings.bitrate)?;
            let tag_stop = Arc::new(AtomicBool::new(false));
            let thread_tag_stop = tag_stop.clone();
            let tag_status = status.clone();
            let tag_thread = thread::spawn(move || {
                metadata_loop(
                    settings,
                    password,
                    metadata,
                    thread_tag_stop,
                    tag_status,
                    connector,
                )
            });
            let mut output = NetworkWriter {
                socket: &mut socket,
                stop: &stop,
                stop_deadline: None,
            };
            let result = encode_master(
                capture.stream.clone(),
                capture.sample_rate,
                encoder,
                &stop,
                &status,
                &mut output,
            );
            tag_stop.store(true, Ordering::Release);
            let _ = socket.shutdown();
            let _ = tag_thread.join();
            result
        }));
        self.poll();
        Ok(())
    }

    pub fn start_recording(
        &mut self,
        capture: Arc<MasterCapture>,
        settings: &StreamSettings,
    ) -> Result<(), String> {
        if self.recording_busy() {
            return Err("A recording is already running or is still being finalized.".into());
        }
        bitrate(settings.bitrate)?;
        if settings.recording_dir.as_os_str().is_empty() {
            return Err("Choose a recording folder.".into());
        }
        let directory = settings.recording_dir.clone();
        let rate = settings.bitrate;
        self.recording_job = Some(Job::spawn(move |stop, status| {
            let encoder = encoder(capture.sample_rate, rate)?;
            let (path, file) = recording_file(&directory)?;
            update(&status, |status| status.path = Some(path));
            let mut file = BufWriter::new(file);
            let result = encode_master(
                capture.recording.clone(),
                capture.sample_rate,
                encoder,
                &stop,
                &status,
                &mut file,
            );
            let finalized = file
                .flush()
                .and_then(|_| file.get_ref().sync_all())
                .map_err(|e| format!("Could not finalize MP3: {e}"));
            result.and(finalized)
        }));
        self.poll();
        Ok(())
    }

    pub fn stop_stream(&self) {
        if let Some(job) = &self.stream_job {
            job.stop();
        }
    }
    pub fn stop_recording(&self) {
        if let Some(job) = &self.recording_job {
            job.stop();
        }
    }

    pub fn set_metadata(&self, enabled: bool, title: &str) {
        let mut metadata = self.metadata.lock().unwrap_or_else(|e| e.into_inner());
        metadata.enabled = enabled;
        metadata.title = title
            .chars()
            .filter(|c| !c.is_control())
            .take(512)
            .collect();
    }

    pub fn poll(&mut self) {
        for (job, status) in [
            (&mut self.stream_job, &mut self.stream_status),
            (&mut self.recording_job, &mut self.recording_status),
        ] {
            if let Some(current) = job {
                *status = current
                    .status
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if current.thread.is_finished() {
                    let finished = job.take().unwrap();
                    if finished.thread.join().is_err() {
                        status.phase = Phase::Error;
                        status.message =
                            "The encoder background thread stopped unexpectedly.".into();
                    } else {
                        *status = finished
                            .status
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone();
                    }
                }
            }
        }
    }
}

impl Drop for Broadcaster {
    fn drop(&mut self) {
        self.stop_stream();
        self.stop_recording();
        for job in [self.stream_job.take(), self.recording_job.take()]
            .into_iter()
            .flatten()
        {
            let _ = job.thread.join();
        }
    }
}

fn bitrate(value: u16) -> Result<Bitrate, String> {
    match value {
        128 => Ok(Bitrate::Kbps128),
        192 => Ok(Bitrate::Kbps192),
        256 => Ok(Bitrate::Kbps256),
        320 => Ok(Bitrate::Kbps320),
        _ => Err("Choose an MP3 bitrate of 128, 192, 256 or 320 kbit/s.".into()),
    }
}

fn encoder(rate: u32, kbps: u16) -> Result<Encoder, String> {
    let configure = || -> Result<Encoder, mp3lame_encoder::BuildError> {
        Builder::new()
            .ok_or(mp3lame_encoder::BuildError::Other(-1))?
            .with_num_channels(2)?
            .with_sample_rate(rate)?
            .with_output_sample_rate(NonZeroU32::new(44100))?
            .with_brate(bitrate(kbps).expect("validated bitrate"))?
            .with_vbr_mode(VbrMode::Off)?
            .with_to_write_vbr_tag(false)?
            .with_quality(Quality::NearBest)?
            .build()
    };
    configure().map_err(|e| format!("Could not start MP3 encoder: {e}"))
}

struct CaptureSession(Arc<PcmBuffer>);
impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.0.stop();
    }
}

fn encode_master(
    buffer: Arc<PcmBuffer>,
    rate: u32,
    mut encoder: Encoder,
    stop: &AtomicBool,
    status: &SharedStatus,
    output: &mut impl Write,
) -> Result<(), String> {
    let _session = CaptureSession(buffer.clone());
    let mut pcm = Vec::with_capacity(4096);
    let mut mp3 = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(2048).max(7200));
    buffer.begin();
    update(status, |status| {
        status.phase = Phase::Running;
        status.message = "Processing master audio".into();
    });
    let mut frames = 0_u64;
    let mut bytes = 0_u64;
    let mut ending = false;
    let mut overflow = false;
    loop {
        if !ending && (stop.load(Ordering::Acquire) || buffer.dropped() != 0) {
            overflow = buffer.dropped() != 0;
            buffer.stop();
            ending = true;
        }
        let count = buffer.drain(&mut pcm, 2048);
        if count == 0 {
            if ending {
                break;
            }
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        mp3.clear();
        encoder
            .encode_to_vec(InterleavedPcm(&pcm), &mut mp3)
            .map_err(|e| format!("MP3 encoding failed: {e}"))?;
        output
            .write_all(&mp3)
            .map_err(|e| format!("MP3 output interrupted: {e}"))?;
        frames += count as u64;
        bytes += mp3.len() as u64;
        update(status, |status| {
            status.bytes = bytes;
            status.seconds = frames as f64 / rate as f64;
        });
    }
    mp3.clear();
    encoder
        .flush_to_vec::<FlushGap>(&mut mp3)
        .map_err(|e| format!("MP3 finalization failed: {e}"))?;
    output
        .write_all(&mp3)
        .map_err(|e| format!("Could not write MP3 finalization data: {e}"))?;
    update(status, |status| status.bytes = bytes + mp3.len() as u64);
    if overflow {
        Err("Master buffer overflow; output stopped. The encoder, server or disk could not keep up.".into())
    } else {
        Ok(())
    }
}

fn recording_file(directory: &Path) -> Result<(PathBuf, File), String> {
    fs::create_dir_all(directory).map_err(|e| format!("Recording folder unavailable: {e}"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for suffix in 0..1000 {
        let path = directory.join(format!("RustDJ-Master-{stamp}-{suffix:03}.mp3"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("Could not create recording: {error}")),
        }
    }
    Err("No unused recording filename is available.".into())
}

fn open_source(
    connector: &Connector,
    settings: &StreamSettings,
    password: &str,
    stop: &AtomicBool,
) -> Result<Connection, String> {
    settings.validate(password)?;
    let mut socket = connector.connect(
        &settings.host,
        settings.source_port,
        settings.security,
        stop,
    )?;
    authenticate(&mut socket, settings, password, stop)?;
    Ok(socket)
}

fn authenticate(
    socket: &mut Connection,
    settings: &StreamSettings,
    password: &str,
    stop: &AtomicBool,
) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        return Err("Connection cancelled".into());
    }
    let auth = if settings.stream_id == 1 {
        password.to_owned()
    } else {
        format!("{password}:#{}", settings.stream_id)
    };
    socket
        .write_all(format!("{auth}\r\n").as_bytes())
        .map_err(|_| "Could not send source login.".to_owned())?;
    let response = read_response(socket, stop, true)?;
    if response.lines().next().unwrap_or("").trim() != "OK2" {
        return Err(
            "SHOUTcast login rejected. Check the source password, port and stream ID.".into(),
        );
    }
    let headers = format!(
        "icy-name:{}\r\nicy-genre:{}\r\nicy-url:{}\r\nicy-pub:0\r\nicy-br:{}\r\ncontent-type:audio/mpeg\r\nUser-Agent:RustDJ/{}\r\n\r\n",
        settings.station,
        settings.genre,
        settings.url,
        settings.bitrate,
        env!("CARGO_PKG_VERSION")
    );
    socket
        .write_all(headers.as_bytes())
        .map_err(|_| "Could not register SHOUTcast stream details.".into())
}

struct NetworkWriter<'a> {
    socket: &'a mut Connection,
    stop: &'a AtomicBool,
    stop_deadline: Option<Instant>,
}
impl Write for NetworkWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            self.check_deadline(deadline)?;
            match self.socket.flush() {
                Err(error) if retryable(&error) => thread::sleep(Duration::from_millis(2)),
                result => return result,
            }
        }
    }
    fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !bytes.is_empty() {
            self.check_deadline(deadline)?;
            match self.socket.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl NetworkWriter<'_> {
    fn check_deadline(&mut self, deadline: Instant) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire) {
            let end = self
                .stop_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(1));
            if Instant::now() >= *end {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Stream stopped"));
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The stream server is not accepting audio",
            ));
        }
        Ok(())
    }
}

fn read_response(
    socket: &mut Connection,
    stop: &AtomicBool,
    line_only: bool,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    while bytes.len() < 16384 {
        if stop.load(Ordering::Acquire) {
            return Err("Connection cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("SHOUTcast response is missing or timed out.".into());
        }
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                bytes.extend_from_slice(&chunk[..count]);
                if line_only && bytes.contains(&b'\n') {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                thread::sleep(Duration::from_millis(2))
            }
            Err(_) => return Err("Could not read SHOUTcast response.".into()),
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn percent(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn send_metadata(
    connector: &Connector,
    settings: &StreamSettings,
    password: &str,
    title: &str,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut socket =
        connector.connect(&settings.host, settings.admin_port, settings.security, stop)?;
    let host = settings.host.trim();
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let request = format!(
        "GET /admin.cgi?sid={}&mode=updinfo&pass={}&song={} HTTP/1.0\r\nHost: {}:{}\r\nUser-Agent: RustDJ/{}\r\nConnection: close\r\n\r\n",
        settings.stream_id,
        percent(password),
        percent(title),
        host,
        settings.admin_port,
        env!("CARGO_PKG_VERSION")
    );
    socket
        .write_all(request.as_bytes())
        .map_err(|_| "Could not send track titles.".to_owned())?;
    let response = read_response(&mut socket, stop, false)?;
    let first = response.lines().next().unwrap_or("");
    let body = response.to_ascii_lowercase();
    if first.split_whitespace().nth(1) != Some("200")
        || [
            "invalid password",
            "invalid token",
            "unauthorized",
            "not authorized",
            "authentication failed",
        ]
        .iter()
        .any(|text| body.contains(text))
    {
        return Err(
            "The server rejected track titles. Check the metadata port and source password.".into(),
        );
    }
    Ok(())
}

fn metadata_loop(
    settings: StreamSettings,
    password: String,
    metadata: Arc<Mutex<Metadata>>,
    stop: Arc<AtomicBool>,
    status: SharedStatus,
    connector: Connector,
) {
    let mut previous = None;
    let mut last_attempt = Instant::now() - Duration::from_secs(3);
    while !stop.load(Ordering::Acquire) {
        let next = {
            let metadata = metadata.lock().unwrap_or_else(|e| e.into_inner());
            (metadata.enabled && !metadata.title.is_empty()).then(|| metadata.title.clone())
        };
        if let Some(title) = next {
            if previous.as_ref() != Some(&title) && last_attempt.elapsed() >= Duration::from_secs(2)
            {
                last_attempt = Instant::now();
                let result = send_metadata(&connector, &settings, &password, &title, &stop);
                update(&status, |status| {
                    status.metadata_error = result.as_ref().err().cloned()
                });
                if result.is_ok() {
                    previous = Some(title);
                }
            }
        } else {
            previous = None;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod security_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;
    use std::{
        io::{BufRead, BufReader, Cursor},
        net::TcpListener,
    };

    fn wait(broadcaster: &mut Broadcaster, condition: impl Fn(&Broadcaster) -> bool) {
        let end = Instant::now() + Duration::from_secs(6);
        loop {
            broadcaster.poll();
            if condition(broadcaster) {
                break;
            }
            assert!(
                Instant::now() < end,
                "worker timeout: {:?}, {:?}",
                broadcaster.stream_status,
                broadcaster.recording_status
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn streamer16_local_shoutcast_login_mp3_optional_tags_and_independent_failures() {
        for enabled in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let source_port = listener.local_addr().unwrap().port();
            let source_thread = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut password = String::new();
                reader.read_line(&mut password).unwrap();
                socket.write_all(b"OK2\r\nicy-caps:11\r\n\r\n").unwrap();
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line.trim().is_empty() {
                        break;
                    }
                    headers.push_str(&line);
                }
                let mut mp3 = Vec::new();
                reader.read_to_end(&mut mp3).unwrap();
                (password, headers, mp3)
            });
            let admin = TcpListener::bind("127.0.0.1:0").unwrap();
            let admin_port = admin.local_addr().unwrap().port();
            admin.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::<String>::new()));
            let thread_requests = requests.clone();
            let end_admin = Arc::new(AtomicBool::new(false));
            let thread_end = end_admin.clone();
            let admin_thread = thread::spawn(move || {
                while !thread_end.load(Ordering::Acquire) {
                    match admin.accept() {
                        Ok((mut socket, _)) => {
                            socket
                                .set_read_timeout(Some(Duration::from_secs(1)))
                                .unwrap();
                            let mut reader = BufReader::new(socket.try_clone().unwrap());
                            let mut request = String::new();
                            loop {
                                let mut line = String::new();
                                if reader.read_line(&mut line).unwrap() == 0
                                    || line.trim().is_empty()
                                {
                                    break;
                                }
                                request.push_str(&line);
                            }
                            thread_requests.lock().unwrap().push(request);
                            socket
                                .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                                .unwrap();
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => panic!("mock admin: {error}"),
                    }
                }
            });
            let settings = StreamSettings {
                host: "127.0.0.1".into(),
                security: ConnectionSecurity::Plaintext,
                source_port,
                admin_port,
                stream_id: 2,
                send_tags: enabled,
                ..StreamSettings::default()
            };
            let password = "local-test&p?";
            let title = "Björk & Test ? / Live";
            let capture = Arc::new(MasterCapture::new(44100));
            let mut broadcaster = Broadcaster::default();
            broadcaster.set_metadata(enabled, title);
            broadcaster
                .start_stream(capture.clone(), settings.clone(), password.into())
                .unwrap();
            wait(&mut broadcaster, |b| {
                b.stream_status.phase == Phase::Running
            });
            for i in 0..22050 {
                let sample = (std::f32::consts::TAU * 440.0 * i as f32 / 44100.0).sin() * 0.2;
                capture.push([sample, -sample]);
            }
            wait(&mut broadcaster, |b| b.stream_status.seconds >= 0.5);
            let deadline = Instant::now() + Duration::from_secs(3);
            if enabled {
                while requests.lock().unwrap().is_empty() {
                    assert!(Instant::now() < deadline, "missing metadata update");
                    thread::sleep(Duration::from_millis(5));
                }
            } else {
                thread::sleep(Duration::from_millis(250));
            }
            broadcaster.stop_stream();
            wait(&mut broadcaster, |b| !b.stream_busy());
            assert_eq!(
                broadcaster.stream_status.phase,
                Phase::Done,
                "{:?}",
                broadcaster.stream_status
            );
            let (auth, headers, mp3) = source_thread.join().unwrap();
            assert_eq!(auth, format!("{password}:#2\r\n"));
            assert!(headers.contains("content-type:audio/mpeg"));
            assert!(headers.contains("icy-br:192"));
            assert!(headers.contains("icy-pub:0"));
            assert_eq!(mp3.len() as u64, broadcaster.stream_status.bytes);
            let decoder = rodio::Decoder::try_from(Cursor::new(mp3)).unwrap();
            assert_eq!(decoder.channels(), 2);
            assert_eq!(decoder.sample_rate(), 44100);
            assert!(decoder.count() > 44100);
            end_admin.store(true, Ordering::Release);
            admin_thread.join().unwrap();
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), usize::from(enabled));
            if enabled {
                assert!(requests[0].contains("sid=2&mode=updinfo"));
                assert!(requests[0].contains(&format!("pass={}", percent(password))));
                assert!(requests[0].contains(&format!("song={}", percent(title))));
                assert!(broadcaster.stream_status.metadata_error.is_none());
            }
            assert!(!serde_json::to_string(&settings).unwrap().contains(password));
            let mut invalid = settings;
            invalid.station = "bad\r\nInjected: header".into();
            assert!(
                broadcaster
                    .start_stream(capture, invalid, password.into())
                    .is_err()
            );
        }

        // A rejected source login must leave an independent recording running.
        let directory = tempfile::Builder::new()
            .prefix("streamer16-error-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("work"))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let source_port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            socket.write_all(b"invalid password\r\n").unwrap();
        });
        let settings = StreamSettings {
            host: "127.0.0.1".into(),
            security: ConnectionSecurity::Plaintext,
            source_port,
            recording_dir: directory.path().into(),
            ..StreamSettings::default()
        };
        let capture = Arc::new(MasterCapture::new(44100));
        let mut broadcaster = Broadcaster::default();
        broadcaster
            .start_recording(capture.clone(), &settings)
            .unwrap();
        wait(&mut broadcaster, |b| {
            b.recording_status.phase == Phase::Running
        });
        broadcaster
            .start_stream(capture.clone(), settings, "rejected-test-secret".into())
            .unwrap();
        wait(&mut broadcaster, |b| !b.stream_busy());
        assert_eq!(broadcaster.stream_status.phase, Phase::Error);
        assert!(
            !broadcaster
                .stream_status
                .message
                .contains("rejected-test-secret")
        );
        assert!(broadcaster.recording_busy());
        for _ in 0..11025 {
            capture.push([0.15, -0.15]);
        }
        broadcaster.stop_recording();
        wait(&mut broadcaster, |b| !b.recording_busy());
        assert_eq!(broadcaster.recording_status.phase, Phase::Done);
        assert!(
            crate::track::open_decoder(broadcaster.recording_status.path.as_ref().unwrap())
                .unwrap()
                .count()
                > 1000
        );
        server.join().unwrap();

        #[cfg(windows)]
        {
            let password = "local-persistence-test-Päss&?";
            let encrypted = crate::credentials::protect(password).unwrap();
            assert!(!encrypted.is_empty());
            assert_ne!(encrypted, password.as_bytes());
            let saved = crate::settings::Settings {
                streamer: StreamSettings {
                    host: "radio.example.invalid".into(),
                    source_port: 9021,
                    admin_port: 9020,
                    stream_id: 3,
                    station: "Meine Station".into(),
                    bitrate: 256,
                    send_tags: true,
                    automatic_title: false,
                    manual_title: "Live aus RustDJ".into(),
                    recording_dir: directory.path().into(),
                    protected_password: encrypted,
                    ..StreamSettings::default()
                },
                ..crate::settings::Settings::default()
            };
            let path = directory.path().join("settings.json");
            crate::settings::save_to(&path, &saved).unwrap();
            let content = fs::read_to_string(&path).unwrap();
            assert!(!content.contains(password));
            let (mut restored, error) = crate::settings::load_from(&path);
            assert!(error.is_none());
            assert_eq!(restored.streamer, saved.streamer);
            assert_eq!(
                crate::credentials::unprotect(&restored.streamer.protected_password).unwrap(),
                password
            );
            restored.streamer.host = "zweiter.example.invalid".into();
            crate::settings::save_to(&path, &restored).unwrap();
            let (restored, error) = crate::settings::load_from(&path);
            assert!(error.is_none());
            assert_eq!(restored.streamer.host, "zweiter.example.invalid");
            assert_eq!(
                crate::credentials::unprotect(&restored.streamer.protected_password).unwrap(),
                password
            );
            assert!(
                crate::credentials::unprotect(&restored.streamer.protected_password[..5]).is_err()
            );
            assert!(crate::credentials::protect("").unwrap().is_empty());
            assert_eq!(crate::credentials::unprotect(&[]).unwrap(), "");
        }
    }
}
