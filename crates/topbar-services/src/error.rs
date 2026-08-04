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
        ];
        for error in errors {
            let message = error.user_message();
            assert!(message.len() < 60, "{message} is too long for a toast");
            assert!(!message.contains("niri"), "{message} leaks the transport");
            assert!(!message.is_empty());
        }
    }

    #[test]
    fn display_keeps_the_detail_the_log_needs() {
        let error = SvcError::Rejected("no such workspace".into());
        assert!(error.to_string().contains("no such workspace"));
    }
}
