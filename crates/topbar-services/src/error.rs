//! The error every mutating service call returns.
//!
//! Two audiences, one type: [`Display`](std::fmt::Display) carries the detail a
//! log needs, and [`SvcError::user_message`] carries the one short sentence a
//! toast or an inline row can show. Widgets never format the detail themselves.

use std::time::Duration;

/// Something a service could not do.
#[derive(Debug, thiserror::Error)]
pub enum SvcError {
    /// `$NIRI_SOCKET` is not set, so there is no compositor to talk to.
    #[error("NIRI_SOCKET is not set; the panel is not running under niri")]
    NoNiriSocket,

    /// The socket could not be reached, or the connection broke mid-request.
    #[error("niri socket I/O failed: {0}")]
    Io(#[source] std::io::Error),

    /// The compositor did not answer in time.
    #[error("niri did not answer within {0:?}")]
    Timeout(Duration),

    /// The compositor answered, refusing the request.
    #[error("niri refused the request: {0}")]
    Rejected(String),

    /// The compositor answered with something we cannot read.
    #[error("unreadable reply from niri: {0}")]
    Protocol(String),

    /// The session bus could not be reached, or refused a request.
    ///
    /// Carries the detail as text rather than a `zbus::Error` because it is
    /// published through a watch channel, and every value in one has to clone.
    #[error("session bus error: {0}")]
    Bus(String),

    /// Another process owns a well-known name the panel needs.
    #[error("{0} is owned by another process")]
    NameTaken(String),

    /// The notification the panel was asked to act on has gone.
    #[error("notification {0} no longer exists")]
    GoneNotification(u32),

    /// There is no media player to act on, or the one named has gone.
    #[error("no media player answered: {0}")]
    NoPlayer(String),

    /// A service task has stopped, so its commands go nowhere.
    #[error("the {0} service is not running")]
    ServiceStopped(&'static str),
}

impl SvcError {
    /// One short sentence, written for the user rather than the log.
    ///
    /// Deliberately free of detail: the specifics belong in the log line that
    /// accompanies the toast, not in the toast.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NoNiriSocket | Self::Io(_) | Self::Timeout(_) => "Could not reach the compositor",
            Self::Rejected(_) => "The compositor refused the request",
            Self::Protocol(_) => "The compositor sent an unexpected reply",
            Self::Bus(_) => "Could not reach the session bus",
            Self::NameTaken(_) => "Another notification daemon is running",
            Self::GoneNotification(_) => "That notification is no longer available",
            Self::NoPlayer(_) => "No media player is available",
            Self::ServiceStopped(_) => "That part of the panel has stopped",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_messages_are_short_and_detail_free() {
        let errors = [
            SvcError::NoNiriSocket,
            SvcError::Io(std::io::Error::other("connection refused")),
            SvcError::Timeout(Duration::from_secs(2)),
            SvcError::Rejected("no such workspace".into()),
            SvcError::Protocol("expected Handled".into()),
            SvcError::Bus("connection refused".into()),
            SvcError::NameTaken("org.freedesktop.Notifications".into()),
            SvcError::GoneNotification(7),
            SvcError::NoPlayer("org.mpris.MediaPlayer2.spotify".into()),
            SvcError::ServiceStopped("notifications"),
        ];
        for error in errors {
            let message = error.user_message();
            assert!(message.len() < 60, "{message} is too long for a toast");
            assert!(!message.contains("niri"), "{message} leaks the transport");
            assert!(
                !message.contains("org.freedesktop"),
                "{message} leaks a bus name"
            );
            assert!(!message.is_empty());
        }
    }

    #[test]
    fn display_keeps_the_detail_the_log_needs() {
        let error = SvcError::Rejected("no such workspace".into());
        assert!(error.to_string().contains("no such workspace"));
    }
}
