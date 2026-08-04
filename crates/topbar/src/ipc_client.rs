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
pub const UNREACHABLE: &str = "could not reach gnome-topbar IPC socket (is the panel running?)";

const TIMEOUT: Duration = Duration::from_secs(2);

/// Path of the panel's IPC socket.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join(SOCKET_NAME))
}

/// Connect to the panel and complete the version handshake.
fn connect() -> Result<UnixStream, String> {
    let path = socket_path().ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;
    let mut stream = UnixStream::connect(&path).map_err(|err| err.to_string())?;
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

/// Check whether a panel is listening, without sending a command.
pub fn probe() -> Result<(), String> {
    connect().map(|_| ())
}

/// Send one request to the running panel and return its response.
pub fn request(request: &IpcRequest) -> Result<IpcResponse, String> {
    let mut stream = connect()?;
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
