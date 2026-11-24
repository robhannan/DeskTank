//! Single-instance lock and local, one-line command protocol (SPEC §4.3).

#[cfg(unix)]
use std::fs;
use std::fs::{DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use desktank_platform::runtime_dir;
use desktank_sim::cheats::FEED_FOOD;
use desktank_sim::{MaterialId, MaterialTable};
use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream, prelude::*};
use thiserror::Error;

use crate::InteractState;

const MAX_LINE: usize = 1024;
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

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
        wake: impl Fn(Request, Sender<Reply>) -> bool + Send + 'static,
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
        // TODO: install a current-user-only DACL once we can obtain the
        // current user's SID safely. Interprocess uses the default DACL for now.
        let (instance, listener) = (Self { _lock: lock }, options.create_sync()?);

        std::thread::Builder::new()
            .name("desktank-ipc".to_owned())
            .spawn(move || serve(&listener, &materials, wake))?;
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

fn serve(
    listener: &Listener,
    materials: &MaterialTable,
    wake: impl Fn(Request, Sender<Reply>) -> bool,
) {
    loop {
        let mut stream = match listener.accept() {
            Ok(stream) => stream,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("desktank window: accepting IPC connection: {error}");
                break;
            }
        };
        // Interprocess's blocking Windows pipes do not support timeouts.
        #[cfg(unix)]
        if stream.set_recv_timeout(Some(REPLY_TIMEOUT)).is_err()
            || stream.set_send_timeout(Some(REPLY_TIMEOUT)).is_err()
        {
            continue;
        }
        let reply = match read_line(&stream) {
            Ok(line) => match parse_request(&line, materials) {
                Ok(request) => {
                    let (tx, rx) = mpsc::channel();
                    // The event loop has gone: tell this client, then stop.
                    let reply = if wake(request, tx) {
                        rx.recv_timeout(REPLY_TIMEOUT)
                    } else {
                        Err(RecvTimeoutError::Disconnected)
                    };
                    match reply {
                        Ok(reply) => reply,
                        Err(RecvTimeoutError::Timeout) => Err("app busy"),
                        Err(RecvTimeoutError::Disconnected) => {
                            let _ = stream.write_all(format_reply(&Err("app exiting")).as_bytes());
                            break;
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
    #[cfg(unix)]
    {
        let timeout = Some(REPLY_TIMEOUT + Duration::from_secs(1));
        stream.set_recv_timeout(timeout)?;
        stream.set_send_timeout(timeout)?;
    }
    writeln!(stream, "{request}")?;
    let line = read_line(&stream)?;
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
    use super::*;

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
