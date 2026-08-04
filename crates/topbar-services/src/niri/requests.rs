//! Sending actions to niri.
//!
//! Actions cannot share the event-stream connection: asking for an event
//! stream consumes the socket and shuts down its write half, so a click on a
//! workspace would have nowhere to go. This is a second, separate connection.
//!
//! niri answers any number of sequential requests on one connection, so the
//! handle keeps a single one alive behind a mutex instead of paying a connect
//! per click. A broken connection is dropped and re-established once, which
//! covers the common case of the compositor having restarted since the last
//! action.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use niri_ipc::{Action, LayoutSwitchTarget, Reply, Request, Response, WorkspaceReferenceArg};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_util::codec::{Framed, LinesCodec};
use tracing::debug;

use crate::error::SvcError;

/// How long any single action may take, connection included.
///
/// Short on purpose: these run from a click, and a compositor that has not
/// answered in two seconds is not going to.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// A request connection to niri, cheap to clone and safe to share.
#[derive(Clone)]
pub struct NiriHandle {
    inner: Arc<Inner>,
}

struct Inner {
    /// `None` when `$NIRI_SOCKET` was not set at start-up.
    socket: Option<PathBuf>,
    connection: Mutex<Option<Connection>>,
}

type Connection = Framed<UnixStream, LinesCodec>;

impl NiriHandle {
    /// Create a handle for `socket`, or a handle that always fails if there is
    /// none.
    pub(crate) fn new(socket: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                socket,
                connection: Mutex::new(None),
            }),
        }
    }

    /// Make `id` the active workspace on its output.
    ///
    /// Addressed by id rather than index: indices renumber when workspaces are
    /// re-ordered, so an index captured when the bar was drawn can point at a
    /// different workspace by the time the click lands.
    pub async fn focus_workspace(&self, id: u64) -> Result<(), SvcError> {
        self.act(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Id(id),
        })
        .await
    }

    /// Switch to the next configured keyboard layout.
    pub async fn switch_layout_next(&self) -> Result<(), SvcError> {
        self.act(Action::SwitchLayout {
            layout: LayoutSwitchTarget::Next,
        })
        .await
    }

    /// Switch to the previous configured keyboard layout.
    pub async fn switch_layout_prev(&self) -> Result<(), SvcError> {
        self.act(Action::SwitchLayout {
            layout: LayoutSwitchTarget::Prev,
        })
        .await
    }

    /// Ask niri to exit. Used by the Quick Settings log-out action (M9).
    pub async fn quit_compositor(&self) -> Result<(), SvcError> {
        self.act(Action::Quit {
            skip_confirmation: true,
        })
        .await
    }

    /// Perform `action`, mapping a non-`Handled` answer to an error.
    async fn act(&self, action: Action) -> Result<(), SvcError> {
        match self.request(Request::Action(action)).await? {
            Response::Handled => Ok(()),
            other => Err(SvcError::Protocol(format!(
                "expected Handled, got {other:?}"
            ))),
        }
    }

    /// Send `request` and return niri's response.
    async fn request(&self, request: Request) -> Result<Response, SvcError> {
        let payload =
            serde_json::to_string(&request).map_err(|e| SvcError::Protocol(e.to_string()))?;
        match timeout(REQUEST_TIMEOUT, self.exchange(payload)).await {
            Ok(result) => result,
            Err(_elapsed) => {
                // A timed-out connection is in an unknown state: whatever the
                // reply was, it is still queued on it. Throw it away.
                *self.inner.connection.lock().await = None;
                Err(SvcError::Timeout(REQUEST_TIMEOUT))
            }
        }
    }

    /// One request/reply round trip, retried once on a fresh connection.
    async fn exchange(&self, payload: String) -> Result<Response, SvcError> {
        let socket = self.inner.socket.as_ref().ok_or(SvcError::NoNiriSocket)?;
        let mut guard = self.inner.connection.lock().await;

        let mut reply = None;
        if let Some(connection) = guard.as_mut() {
            match round_trip(connection, &payload).await {
                Ok(value) => reply = Some(value),
                Err(error) => {
                    debug!("the niri request connection went away ({error}); reconnecting");
                    *guard = None;
                }
            }
        }

        let reply = match reply {
            Some(reply) => reply,
            None => {
                let stream = UnixStream::connect(socket).await.map_err(SvcError::Io)?;
                let mut connection = Framed::new(stream, LinesCodec::new());
                let reply = round_trip(&mut connection, &payload).await?;
                *guard = Some(connection);
                reply
            }
        };

        reply.map_err(SvcError::Rejected)
    }
}

/// Write one request and read exactly one reply line.
async fn round_trip(connection: &mut Connection, payload: &str) -> Result<Reply, SvcError> {
    connection
        .send(payload.to_string())
        .await
        .map_err(|e| SvcError::Protocol(format!("could not send the request: {e}")))?;

    let line = connection
        .next()
        .await
        .ok_or_else(|| SvcError::Protocol("niri closed the connection".into()))?
        .map_err(|e| SvcError::Protocol(format!("could not read the reply: {e}")))?;

    serde_json::from_str(&line).map_err(|e| SvcError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire form matters: these strings are what niri parses.
    #[test]
    fn actions_serialise_the_way_niri_expects() {
        let focus = serde_json::to_string(&Request::Action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Id(7),
        }))
        .expect("serialisable");
        assert_eq!(
            focus,
            r#"{"Action":{"FocusWorkspace":{"reference":{"Id":7}}}}"#
        );

        let layout = serde_json::to_string(&Request::Action(Action::SwitchLayout {
            layout: LayoutSwitchTarget::Next,
        }))
        .expect("serialisable");
        assert_eq!(layout, r#"{"Action":{"SwitchLayout":{"layout":"Next"}}}"#);

        // niri answers actions with `Handled`; anything else is a bug on one
        // side or the other.
        let reply: Reply = serde_json::from_str(r#"{"Ok":"Handled"}"#).expect("parsable");
        assert!(matches!(reply, Ok(Response::Handled)));
    }

    #[test]
    fn a_rejection_carries_niris_own_words() {
        let reply: Reply =
            serde_json::from_str(r#"{"Err":"no workspace with id 999"}"#).expect("parsable");
        let error = reply.map_err(SvcError::Rejected).unwrap_err();
        assert!(error.to_string().contains("no workspace with id 999"));
        assert_eq!(error.user_message(), "The compositor refused the request");
    }

    #[tokio::test]
    async fn without_a_socket_every_action_fails_clearly() {
        let handle = NiriHandle::new(None);
        let error = handle.focus_workspace(1).await.unwrap_err();
        assert!(matches!(error, SvcError::NoNiriSocket));
        assert_eq!(error.user_message(), "Could not reach the compositor");
    }

    #[tokio::test]
    async fn a_missing_socket_file_is_an_io_error_not_a_panic() {
        let handle = NiriHandle::new(Some(PathBuf::from(
            "/nonexistent/gnome-topbar/niri-test.sock",
        )));
        let error = handle.switch_layout_next().await.unwrap_err();
        assert!(matches!(error, SvcError::Io(_)), "{error}");
    }
}
