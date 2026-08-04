//! CLI side of the panel IPC socket.
//!
//! M0 only needs enough of this to give every subcommand a real, honest exit
//! path: connect, handshake, send one framed request, read one framed response.
//! The panel side of the socket lands in M8.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use topbar_core::ipc::{
    HEADER_LEN, IpcRequest, IpcResponse, MAX_FRAME_LEN, PROTOCOL_VERSION, SOCKET_NAME,
    decode_frame, encode_frame,
};

/// Message printed whenever the panel cannot be reached.
pub const UNREACHABLE: &str = "could not reach topbar (is the panel running?)";

/// How long to wait for the panel to answer.
///
/// Matches the panel's own answer timeout, so a command that the panel gave up
/// on and one the client gave up on cannot disagree about which happened.
/// Two seconds was not enough: building a control panel for the first time is
/// real work, and `topbar popover show clock` was timing out on a popover that
/// then opened perfectly well.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Path of the panel's IPC socket.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join(SOCKET_NAME))
}

/// Connect to the panel and complete the version handshake.
fn connect(path: &std::path::Path) -> Result<UnixStream, String> {
    let mut stream = UnixStream::connect(path).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|err| err.to_string())?;

    send(
        &mut stream,
        &IpcRequest::Hello {
            version: PROTOCOL_VERSION,
        },
    )?;
    match receive(&mut stream)? {
        IpcResponse::Hello { version } if version == PROTOCOL_VERSION => Ok(stream),
        IpcResponse::Hello { version } => Err(format!(
            "panel speaks IPC protocol v{version}, this binary speaks v{PROTOCOL_VERSION}"
        )),
        other => Err(format!("unexpected handshake response: {other:?}")),
    }
}

/// Send one request to the running panel and return its response.
pub fn request(request: &IpcRequest) -> Result<IpcResponse, String> {
    let path = socket_path().ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;
    request_at(&path, request)
}

/// The same, against a socket at an explicit path.
///
/// Exists for the end-to-end test, which needs a socket somewhere it may write
/// without touching `$XDG_RUNTIME_DIR`. Everything else about the exchange —
/// the handshake, the framing, the timeouts — is the path `topbar` itself
/// takes, which is the point of testing through here rather than around it.
pub fn request_at(path: &std::path::Path, request: &IpcRequest) -> Result<IpcResponse, String> {
    let mut stream = connect(path)?;
    send(&mut stream, request)?;
    receive(&mut stream)
}

fn send(stream: &mut UnixStream, request: &IpcRequest) -> Result<(), String> {
    let frame = encode_frame(request).map_err(|err| err.to_string())?;
    stream.write_all(&frame).map_err(|err| err.to_string())
}

fn receive(stream: &mut UnixStream) -> Result<IpcResponse, String> {
    let mut header = [0u8; HEADER_LEN];
    stream
        .read_exact(&mut header)
        .map_err(|err| err.to_string())?;
    let len = u32::from_le_bytes(header);
    if len > MAX_FRAME_LEN {
        return Err(format!("panel sent an oversized frame ({len} bytes)"));
    }

    let mut frame = header.to_vec();
    frame.resize(HEADER_LEN + len as usize, 0);
    stream
        .read_exact(&mut frame[HEADER_LEN..])
        .map_err(|err| err.to_string())?;

    decode_frame::<IpcResponse>(&frame)
        .map(|(response, _)| response)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use topbar_core::ipc::{DumpTarget, PopoverAction, VisibilityAction};
    use topbar_services::Runtime;
    use topbar_services::ipc::Ipc;

    use super::*;

    /// A socket path of this test's own.
    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("topbar-client-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a writable temp dir");
        dir.join(SOCKET_NAME)
    }

    /// Serve `answer` on a socket and hand back its path.
    ///
    /// The server is the panel's own listener, so what this exercises is the
    /// real thing on both sides: the CLI's client, the framed protocol, and the
    /// services crate's socket.
    fn panel(path: &std::path::Path, answer: impl Fn(IpcRequest) -> IpcResponse + Send + 'static) {
        // `Ipc::serve` spawns, so it has to be called from inside the runtime —
        // which in the panel it is, because `Services::start` blocks on it.
        let entered = Runtime::handle();
        let _guard = entered.enter();
        let ipc = Ipc::serve(Some(path.to_path_buf()));
        let mut requests = ipc.take_requests().expect("the stream is free");
        Runtime::handle().spawn(async move {
            // `ipc` is held for as long as the loop runs; dropping it would not
            // stop the listener, but keeping it is what a panel does.
            let _ipc = ipc;
            while let Some(envelope) = requests.recv().await {
                let response = answer(envelope.request.clone());
                envelope.answer(response);
            }
        });

        // The listener binds on a task; wait for the socket rather than guess.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(path).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "nothing ever bound to {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_cli_can_drive_a_panel_end_to_end() {
        let path = scratch("roundtrip");
        panel(&path, |request| match request {
            IpcRequest::VolumeChanged { percent, muted } => IpcResponse::Value {
                text: format!("volume {percent} muted {muted}"),
            },
            IpcRequest::Popover {
                action: PopoverAction::Show(widget),
            } => IpcResponse::Value {
                text: format!("opened {widget}"),
            },
            IpcRequest::Bar { action } => IpcResponse::Value {
                text: format!("{action:?}"),
            },
            IpcRequest::Dump { target, json } => IpcResponse::Value {
                text: format!("{target:?} json={json}"),
            },
            IpcRequest::ToggleInhibitor => IpcResponse::Error {
                message: "logind said no".to_string(),
            },
            other => IpcResponse::Error {
                message: format!("unexpected {other:?}"),
            },
        });

        // The handshake happens inside `request_at`; a wrong version would fail
        // here rather than at the command.
        assert_eq!(
            request_at(
                &path,
                &IpcRequest::VolumeChanged {
                    percent: 30,
                    muted: false
                }
            ),
            Ok(IpcResponse::Value {
                text: "volume 30 muted false".to_string()
            })
        );

        assert_eq!(
            request_at(
                &path,
                &IpcRequest::Popover {
                    action: PopoverAction::Show("clock".to_string())
                }
            ),
            Ok(IpcResponse::Value {
                text: "opened clock".to_string()
            })
        );

        assert_eq!(
            request_at(
                &path,
                &IpcRequest::Bar {
                    action: VisibilityAction::Toggle
                }
            ),
            Ok(IpcResponse::Value {
                text: "Toggle".to_string()
            })
        );

        assert_eq!(
            request_at(
                &path,
                &IpcRequest::Dump {
                    target: DumpTarget::All,
                    json: true
                }
            ),
            Ok(IpcResponse::Value {
                text: "All json=true".to_string()
            })
        );

        // A failure the panel reports comes back as one, rather than as a
        // transport error the CLI would print differently.
        assert_eq!(
            request_at(&path, &IpcRequest::ToggleInhibitor),
            Ok(IpcResponse::Error {
                message: "logind said no".to_string()
            })
        );

        let _ = std::fs::remove_dir_all(path.parent().expect("a parent"));
    }

    #[test]
    fn a_socket_nothing_is_listening_on_reads_as_no_panel() {
        let path = scratch("absent");
        let error = request_at(&path, &IpcRequest::Reload).expect_err("nothing is there");
        assert!(!error.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent"));
    }

    #[test]
    fn the_unreachable_message_names_the_panel_and_asks_the_obvious_question() {
        assert!(UNREACHABLE.contains("topbar"));
        assert!(UNREACHABLE.contains("running"));
    }
}
