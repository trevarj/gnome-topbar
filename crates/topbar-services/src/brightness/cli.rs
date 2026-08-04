//! The brightness path that does not need a panel.
//!
//! Same contract as `topbar volume`: the key has to work whether or not the
//! panel is up, so the command finds the backlight and writes it itself. The
//! privilege-safe logind call is tried first and a direct sysfs write is the
//! fallback, exactly as the running service does it — the difference is only
//! that this connection lives for one command.

use crate::brightness::device::{self, Backlight};
use crate::logind::{self, ManagerProxy, SessionProxy};

/// What went wrong, in words a terminal can print.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// This machine has no backlight.
    #[error("no backlight device found (is this a laptop with a supported backlight?)")]
    NoBacklight,
    /// Neither logind nor sysfs would take the write.
    #[error("could not set the brightness: {0}")]
    Refused(String),
}

/// A short-lived connection to whatever can set the backlight.
pub struct BrightnessCli {
    backlight: Backlight,
    session: Option<SessionProxy<'static>>,
}

impl BrightnessCli {
    /// Find the backlight and the best way to write it.
    pub async fn open() -> Result<Self, CliError> {
        let backlight = device::discover().ok_or(CliError::NoBacklight)?;
        Ok(Self {
            session: session().await,
            backlight,
        })
    }

    /// The name of the controller being driven.
    pub fn device(&self) -> &str {
        &self.backlight.name
    }

    /// The current brightness, read from sysfs.
    pub fn percent(&self) -> u32 {
        self.backlight.read().unwrap_or(0)
    }

    /// Set the brightness, returning the percentage that was applied.
    pub async fn set(&self, percent: u32) -> Result<u32, CliError> {
        let percent = percent.min(100);
        let raw = self.backlight.raw(percent);

        if let Some(session) = &self.session {
            match session
                .set_brightness(device::SUBSYSTEM, &self.backlight.name, raw)
                .await
            {
                Ok(()) => return Ok(percent),
                Err(error) => {
                    tracing::debug!("logind refused the brightness ({error}); trying sysfs");
                }
            }
        }

        self.backlight
            .write(raw)
            .map(|()| percent)
            .map_err(|error| CliError::Refused(error.to_string()))
    }

    /// Move the brightness by a signed number of points.
    pub async fn step(&self, delta: i32) -> Result<u32, CliError> {
        let current = self.percent();
        let target = if delta >= 0 {
            current.saturating_add(delta.unsigned_abs()).min(100)
        } else {
            current.saturating_sub(delta.unsigned_abs())
        };
        self.set(target).await
    }
}

/// The logind session to write through, if there is one.
async fn session() -> Option<SessionProxy<'static>> {
    let connection = logind::connect(None).await.ok()?;
    let manager = ManagerProxy::new(&connection).await.ok()?;
    let path = logind::session_path(&manager).await?;
    SessionProxy::builder(&connection)
        .path(path)
        .ok()?
        .build()
        .await
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_says_what_to_do_about_it() {
        for error in [
            CliError::NoBacklight,
            CliError::Refused("permission denied".into()),
        ] {
            let message = error.to_string();
            assert!(
                message.chars().next().is_some_and(char::is_lowercase),
                "`{message}` reads badly after `Error: `"
            );
        }
    }
}
