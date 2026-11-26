//! Single-instance lock and local, one-line command protocol (SPEC §4.3).

#[cfg(unix)]
use std::fs;
use std::fs::{DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use desktank_platform::runtime_dir;
use desktank_sim::cheats::FEED_FOOD;
use desktank_sim::{MaterialId, MaterialTable};
use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream, prelude::*};
use thiserror::Error;

use crate::InteractState;

const MAX_LINE: usize = 1024;
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// Connections handled at once. More are told `app busy` instead of getting a
/// thread, so a flood of idle clients cannot exhaust the process.
const MAX_CONCURRENT_CONNECTIONS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    Interact(InteractState),
    Feed(MaterialId),
    Pause,
    Status,
}

pub(crate) type Reply = Result<String, &'static str>;

fn parse_request(line: &str, materials: &MaterialTable) -> Result<Request, &'static str> {
    if line.len() > MAX_LINE {
        return Err("line exceeds 1 KB");
    }
    if !line.ends_with('\n') {
        return Err("expected a newline");
    }
    let mut words = line.split_ascii_whitespace();
    let request = match words.next() {
        Some("interact") => Request::Interact(match words.next() {
            Some("on") => InteractState::On,
            Some("off") => InteractState::Off,
            Some("toggle") => InteractState::Toggle,
            _ => return Err("expected interact on|off|toggle"),
        }),
        Some("feed") => {
            let name = words.next().unwrap_or(FEED_FOOD);
            let food = materials
                .id(name)
                .and_then(|id| materials.get(id))
                .filter(|material| material.palette.is_some_and(|p| p.top_name() == "food"))
                .ok_or("unknown food")?;
            Request::Feed(food.id)
        }
        Some("pause") => Request::Pause,
        Some("status") => Request::Status,
        _ => return Err("unknown command"),
    };
    if words.next().is_some() {
        return Err("unexpected arguments");
    }
    Ok(request)
}

fn format_reply(reply: &Reply) -> String {
    match reply {
        Ok(text) => format!("ok {text}\n"),
        Err(message) => format!("error {message}\n"),
    }
}

fn read_line(reader: impl Read) -> io::Result<String> {
    let mut line = String::new();
    BufReader::new(reader.take(MAX_LINE as u64 + 1)).read_line(&mut line)?;
    Ok(line)
}

/// Reads from a stream until an overall deadline, not per read: each read gets
/// only the time left, so a peer trickling bytes cannot extend the wait.
#[cfg(unix)]
struct DeadlineReader<'a> {
    stream: &'a Stream,
    deadline: Instant,
}

#[cfg(unix)]
impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_recv_timeout(Some(remaining))?;
        match (&*self.stream).read(buf) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Err(io::ErrorKind::TimedOut.into())
            }
            result => result,
        }
    }
}

/// Reads one line from `stream`, giving up `timeout` after the call.
///
/// Interprocess's blocking Windows pipes do not support timeouts, so there the
/// read can wait as long as the peer stays silent.
#[cfg_attr(not(unix), allow(unused_variables))]
fn read_line_within(stream: &Stream, timeout: Duration) -> io::Result<String> {
    #[cfg(unix)]
    {
        read_line(DeadlineReader {
            stream,
            deadline: Instant::now() + timeout,
        })
    }
    #[cfg(not(unix))]
    {
        read_line(stream)
    }
}

#[cfg(unix)]
const SOCKET_FILE: &str = "desktank.sock";

#[cfg(unix)]
fn endpoint(dir: &Path) -> io::Result<Name<'static>> {
    use interprocess::local_socket::GenericFilePath;
    dir.join(SOCKET_FILE).to_fs_name::<GenericFilePath>()
}

#[cfg(windows)]
fn endpoint(dir: &Path) -> io::Result<Name<'static>> {
    use interprocess::local_socket::GenericNamespaced;

    // Incorporate the profile's runtime path as well as the account name.
    // Hashing keeps the pipe name valid and short even for an override.
    // FNV-1a, not `DefaultHasher`: the name must match across builds made
    // with different Rust versions (a CLI and an older running app).
    let user = std::env::var_os("USERNAME").unwrap_or_default();
    let scope = [
        user.as_encoded_bytes(),
        &[0],
        dir.as_os_str().as_encoded_bytes(),
    ]
    .concat()
    .iter()
    .fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("DeskTank-{scope:016x}").to_ns_name::<GenericNamespaced>()
}

fn create_runtime_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // The directory also protects the socket on systems that do not
        // enforce socket file permissions on connect (including macOS).
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC needs a private runtime directory (mode 0700)",
            ));
        }
    }
    Ok(())
}

/// Owns the lock until the socket path has been removed on exit.
pub(crate) struct Instance {
    #[cfg(unix)]
    socket_path: PathBuf,
    _lock: File,
}

impl Instance {
    /// Returns `None` if another instance holds the lock.
    pub(crate) fn start(
        wake: impl Fn(Request, Sender<Reply>) -> bool + Send + Sync + 'static,
    ) -> anyhow::Result<Option<Self>> {
        let dir = runtime_dir().ok_or_else(|| anyhow::anyhow!("no runtime directory"))?;
        create_runtime_dir(&dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("desktank.lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }

        let materials = MaterialTable::builtin()?;
        let options = ListenerOptions::new().name(endpoint(&dir)?);
        #[cfg(unix)]
        let (instance, listener) = {
            use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};

            let socket_path = dir.join(SOCKET_FILE);
            match fs::symlink_metadata(&socket_path) {
                Ok(meta) if meta.file_type().is_socket() => match fs::remove_file(&socket_path) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        return Err(error.into());
                    }
                    _ => {}
                },
                Ok(_) => anyhow::bail!("{} exists and is not a socket", socket_path.display()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            // The instance unlinks the path before releasing the lock.
            let listener = options.reclaim_name(false).create_sync()?;
            let instance = Self {
                socket_path,
                _lock: lock,
            };
            fs::set_permissions(&instance.socket_path, fs::Permissions::from_mode(0o600))?;
            (instance, listener)
        };
        #[cfg(windows)]
        let (instance, listener) = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt as _;

            let options = options.security_descriptor(pipe_security_descriptor()?);
            (Self { _lock: lock }, options.create_sync()?)
        };

        let materials = Arc::new(materials);
        let wake = Arc::new(wake);
        std::thread::Builder::new()
            .name("desktank-ipc".to_owned())
            .spawn(move || serve(&listener, &materials, &wake, REPLY_TIMEOUT))?;
        Ok(Some(instance))
    }
}

#[cfg(unix)]
impl Drop for Instance {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.socket_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("desktank window: removing IPC socket: {error}");
        }
    }
}

/// A protected DACL with a single allow entry for the current user, so other
/// accounts (and the default DACL's broader grants) cannot open the pipe.
#[cfg(windows)]
fn pipe_security_descriptor()
-> io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::U16CString;

    let sid = desktank_platform::current_user_sid()?;
    // `P` protects the DACL from inheritance; `GA` is generic all.
    let sddl = U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID contains a NUL"))?;
    SecurityDescriptor::deserialize(&sddl)
}

/// Counts live connection workers, releasing the slot when dropped (also on
/// early returns and panics).
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_CONCURRENT_CONNECTIONS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(active)))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve<W>(listener: &Listener, materials: &Arc<MaterialTable>, wake: &Arc<W>, timeout: Duration)
where
    W: Fn(Request, Sender<Reply>) -> bool + Send + Sync + 'static,
{
    let active = Arc::new(AtomicUsize::new(0));
    let exiting = Arc::new(AtomicBool::new(false));
    loop {
        let mut stream = match listener.accept() {
            Ok(stream) => stream,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("desktank window: accepting IPC connection: {error}");
                break;
            }
        };
        if exiting.load(Ordering::Acquire) {
            let _ = reply_now(&mut stream, &Err("app exiting"), timeout);
            break;
        }
        let Some(slot) = ConnectionSlot::acquire(&active) else {
            let _ = reply_now(&mut stream, &Err("app busy"), timeout);
            continue;
        };
        let materials = Arc::clone(materials);
        let wake = Arc::clone(wake);
        let exiting = Arc::clone(&exiting);
        // A failed spawn drops the closure, so the client sees the connection
        // close and the slot is released.
        if let Err(error) = std::thread::Builder::new()
            .name("desktank-ipc-conn".to_owned())
            .spawn(move || {
                let _slot = slot;
                handle_connection(stream, &materials, &*wake, &exiting, timeout);
            })
        {
            eprintln!("desktank window: starting IPC worker: {error}");
        }
    }
}

/// Best-effort single reply, bounded by `timeout` where the platform allows.
#[cfg_attr(not(unix), allow(unused_variables))]
fn reply_now(stream: &mut Stream, reply: &Reply, timeout: Duration) -> io::Result<()> {
    #[cfg(unix)]
    stream.set_send_timeout(Some(timeout))?;
    stream.write_all(format_reply(reply).as_bytes())
}

fn handle_connection(
    mut stream: Stream,
    materials: &MaterialTable,
    wake: &impl Fn(Request, Sender<Reply>) -> bool,
    exiting: &AtomicBool,
    timeout: Duration,
) {
    #[cfg(unix)]
    if stream.set_send_timeout(Some(timeout)).is_err() {
        return;
    }
    let reply = match read_line_within(&stream, timeout) {
        Ok(line) => match parse_request(&line, materials) {
            Ok(request) => {
                let (tx, rx) = mpsc::channel();
                // The event loop has gone: tell this client and stop accepting.
                let reply = if wake(request, tx) {
                    rx.recv_timeout(timeout)
                } else {
                    Err(RecvTimeoutError::Disconnected)
                };
                match reply {
                    Ok(reply) => reply,
                    Err(RecvTimeoutError::Timeout) => Err("app busy"),
                    Err(RecvTimeoutError::Disconnected) => {
                        exiting.store(true, Ordering::Release);
                        Err("app exiting")
                    }
                }
            }
            Err(message) => Err(message),
        },
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Err("expected UTF-8"),
        Err(_) => Err("could not read request"),
    };
    let _ = stream.write_all(format_reply(&reply).as_bytes());
}

#[derive(Debug, Error)]
pub(crate) enum ClientError {
    #[error("DeskTank isn't running")]
    NotRunning,
    #[error("invalid IPC request")]
    InvalidRequest,
    #[error("invalid IPC reply")]
    InvalidReply,
    #[error("{0}")]
    Rejected(String),
    #[error("IPC: {0}")]
    Io(#[from] io::Error),
}

pub(crate) fn send(request: &str) -> Result<String, ClientError> {
    if request.len() >= MAX_LINE || request.contains(['\r', '\n']) {
        return Err(ClientError::InvalidRequest);
    }
    let dir = runtime_dir().ok_or(ClientError::NotRunning)?;
    let mut stream = Stream::connect(endpoint(&dir)?).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => ClientError::NotRunning,
        _ => ClientError::Io(error),
    })?;
    let timeout = REPLY_TIMEOUT + Duration::from_secs(1);
    #[cfg(unix)]
    stream.set_send_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    let line = read_line_within(&stream, timeout)?;
    let reply = line.strip_suffix('\n').ok_or(ClientError::InvalidReply)?;
    if line.len() > MAX_LINE {
        return Err(ClientError::InvalidReply);
    }
    if let Some(text) = reply.strip_prefix("ok ") {
        Ok(text.to_owned())
    } else if let Some(message) = reply.strip_prefix("error ") {
        Err(ClientError::Rejected(message.to_owned()))
    } else {
        Err(ClientError::InvalidReply)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU32;
    use std::time::Instant;

    use super::*;

    /// A listener and serve thread on a fresh, short runtime directory (Unix
    /// socket paths are limited to about 100 bytes).
    struct TestServer {
        dir: PathBuf,
    }

    impl TestServer {
        fn start(timeout: Duration) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "dt{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            create_runtime_dir(&dir).unwrap();
            let listener = ListenerOptions::new()
                .name(endpoint(&dir).unwrap())
                .create_sync()
                .unwrap();
            let materials = Arc::new(MaterialTable::builtin().unwrap());
            let wake = Arc::new(|_request: Request, reply: Sender<Reply>| {
                let _ = reply.send(Ok("test".to_owned()));
                true
            });
            // Detached: it blocks in `accept` until the process exits.
            std::thread::spawn(move || serve(&listener, &materials, &wake, timeout));
            Self { dir }
        }

        fn connect(&self) -> Stream {
            let stream = Stream::connect(endpoint(&self.dir).unwrap()).unwrap();
            // Keep a failing test from hanging.
            #[cfg(unix)]
            stream
                .set_recv_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn stalled_client_does_not_block_others() {
        let server = TestServer::start(REPLY_TIMEOUT);
        let mut stalled = server.connect();
        stalled.write_all(b"stat").unwrap();

        let started = Instant::now();
        let mut stream = server.connect();
        stream.write_all(b"status\n").unwrap();
        let line = read_line(&stream).unwrap();
        assert_eq!(line, "ok test\n");
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(stalled);
    }

    #[cfg(unix)]
    #[test]
    fn trickling_client_hits_the_overall_deadline() {
        let timeout = Duration::from_millis(200);
        let server = TestServer::start(timeout);
        let stream = server.connect();
        let started = Instant::now();
        std::thread::scope(|scope| {
            // One byte every 120 ms never trips a per-read timeout of 200 ms,
            // but the whole line is due after 200 ms.
            scope.spawn(|| {
                for _ in 0..20 {
                    if (&stream).write_all(b"s").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(120));
                }
            });
            let line = read_line(&stream).unwrap();
            assert_eq!(line, "error could not read request\n");
            assert!(started.elapsed() < Duration::from_secs(1));
        });
    }

    #[cfg(windows)]
    #[test]
    fn pipe_descriptor_allows_only_the_current_user() {
        use interprocess::os::windows::security_descriptor::AsSecurityDescriptorExt as _;

        // DACL_SECURITY_INFORMATION
        const DACL: u32 = 0x4;
        let sid = desktank_platform::current_user_sid().unwrap();
        let text = pipe_security_descriptor()
            .unwrap()
            .serialize(DACL, widestring::U16CStr::to_string_lossy)
            .unwrap();
        assert!(text.starts_with("D:P("), "{text}");
        assert_eq!(text.matches('(').count(), 1, "{text}");
        assert!(text.contains(&sid), "{text}");
    }

    #[test]
    fn parses_commands() {
        let materials = MaterialTable::builtin().unwrap();
        for (line, request) in [
            ("interact on\n", Request::Interact(InteractState::On)),
            ("interact off\n", Request::Interact(InteractState::Off)),
            (
                "interact toggle\r\n",
                Request::Interact(InteractState::Toggle),
            ),
            ("pause\n", Request::Pause),
            ("status\n", Request::Status),
            ("feed\n", Request::Feed(materials.id(FEED_FOOD).unwrap())),
            (
                "feed sinking_pellet\n",
                Request::Feed(materials.id("sinking_pellet").unwrap()),
            ),
        ] {
            assert_eq!(parse_request(line, &materials), Ok(request));
        }
    }

    #[test]
    fn rejects_invalid_commands() {
        let materials = MaterialTable::builtin().unwrap();
        for (line, message) in [
            ("quit\n", "unknown command"),
            ("interact maybe\n", "expected interact on|off|toggle"),
            ("interact\n", "expected interact on|off|toggle"),
            ("pause on\n", "unexpected arguments"),
            ("status now\n", "unexpected arguments"),
            ("feed flake_food extra\n", "unexpected arguments"),
            ("feed rocks\n", "unknown food"),
            ("feed sand\n", "unknown food"),
            ("pause", "expected a newline"),
        ] {
            assert_eq!(parse_request(line, &materials), Err(message));
        }
    }

    #[test]
    fn caps_lines_including_newline() {
        let materials = MaterialTable::builtin().unwrap();
        let at_limit = format!("status{}\n", " ".repeat(MAX_LINE - 7));
        assert_eq!(parse_request(&at_limit, &materials), Ok(Request::Status));

        let over_limit = format!("status{}\n", " ".repeat(2 * MAX_LINE));
        let line = read_line(over_limit.as_bytes()).unwrap();
        assert_eq!(line.len(), MAX_LINE + 1);
        assert_eq!(parse_request(&line, &materials), Err("line exceeds 1 KB"));
        assert_eq!(read_line(&b"status\npause\n"[..]).unwrap(), "status\n");
    }

    #[test]
    fn formats_replies() {
        assert_eq!(
            format_reply(&Ok("paused=true".to_owned())),
            "ok paused=true\n"
        );
        assert_eq!(format_reply(&Err("app busy")), "error app busy\n");
    }
}
