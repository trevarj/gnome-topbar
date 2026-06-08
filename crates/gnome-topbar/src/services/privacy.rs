//! Privacy activity monitoring for Quick Settings indicators.
//!
//! Microphone recording is reported by [`AudioService`](crate::services::audio::AudioService).
//! Screen sharing is detected from PipeWire graph state because portal request signals are
//! directed to the requesting application and are not reliably visible to this process.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use gtk4::glib;
use serde_json::Value;
use tracing::{debug, warn};

use super::callbacks::{CallbackId, Callbacks};

const PIPEWIRE_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Privacy-sensitive desktop activity state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivacySnapshot {
    /// Whether a PipeWire screen-cast source is currently linked to a consumer.
    pub screen_sharing: bool,
}

/// Process-wide privacy activity service.
pub struct PrivacyService {
    current: RefCell<PrivacySnapshot>,
    callbacks: Callbacks<PrivacySnapshot>,
    shutdown: Arc<AtomicBool>,
}

impl PrivacyService {
    fn new() -> Rc<Self> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let service = Rc::new(Self {
            current: RefCell::new(PrivacySnapshot::default()),
            callbacks: Callbacks::new(),
            shutdown: Arc::clone(&shutdown),
        });

        spawn_pipewire_monitor(shutdown);
        service
    }

    /// Get the global PrivacyService singleton.
    pub fn global() -> Rc<Self> {
        thread_local! {
            static INSTANCE: Rc<PrivacyService> = PrivacyService::new();
        }

        INSTANCE.with(|s| s.clone())
    }

    /// Register a callback for privacy state changes.
    pub fn connect<F>(&self, callback: F) -> CallbackId
    where
        F: Fn(&PrivacySnapshot) + 'static,
    {
        let id = self.callbacks.register(callback);
        let snapshot = self.current.borrow().clone();
        self.callbacks.notify_single(id, &snapshot);
        id
    }

    /// Unregister a callback by ID.
    pub fn disconnect(&self, id: CallbackId) -> bool {
        self.callbacks.unregister(id)
    }

    /// Get the latest privacy snapshot.
    pub fn snapshot(&self) -> PrivacySnapshot {
        self.current.borrow().clone()
    }

    fn apply_screen_sharing(&self, screen_sharing: bool) {
        let mut current = self.current.borrow_mut();
        if current.screen_sharing == screen_sharing {
            return;
        }

        current.screen_sharing = screen_sharing;
        self.callbacks.notify(&current);
    }
}

impl Drop for PrivacyService {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

fn spawn_pipewire_monitor(shutdown: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut last = None;

        while !shutdown.load(Ordering::Relaxed) {
            let screen_sharing = detect_screen_sharing();
            if last != Some(screen_sharing) {
                last = Some(screen_sharing);
                glib::idle_add_once(move || {
                    PrivacyService::global().apply_screen_sharing(screen_sharing);
                });
            }

            thread::sleep(PIPEWIRE_POLL_INTERVAL);
        }
    });
}

fn detect_screen_sharing() -> bool {
    let output = match Command::new("pw-dump")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            debug!("PrivacyService: pw-dump unavailable: {}", e);
            return false;
        }
    };

    if !output.status.success() {
        debug!("PrivacyService: pw-dump exited with {}", output.status);
        return false;
    }

    let json = match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(json) => json,
        Err(e) => {
            warn!("PrivacyService: failed to parse pw-dump output: {}", e);
            return false;
        }
    };

    screen_sharing_from_pw_dump(&json)
}

fn screen_sharing_from_pw_dump(json: &Value) -> bool {
    let Some(objects) = json.as_array() else {
        return false;
    };

    let mut nodes = HashMap::new();
    let mut active_links = Vec::new();

    for object in objects {
        match object.get("type").and_then(Value::as_str) {
            Some("PipeWire:Interface:Node") => {
                if let Some(id) = object.get("id").and_then(Value::as_u64) {
                    nodes.insert(id, PipeWireNode::from_object(object));
                }
            }
            Some("PipeWire:Interface:Link") => {
                if link_state(object) == Some("active")
                    && let (Some(output), Some(input)) = (
                        object
                            .pointer("/info/output-node-id")
                            .and_then(Value::as_u64),
                        object
                            .pointer("/info/input-node-id")
                            .and_then(Value::as_u64),
                    )
                {
                    active_links.push((output, input));
                }
            }
            _ => {}
        }
    }

    let screen_cast_sources: HashSet<u64> = nodes
        .iter()
        .filter_map(|(id, node)| node.is_screen_cast_source().then_some(*id))
        .collect();

    active_links
        .into_iter()
        .any(|(output, _input)| screen_cast_sources.contains(&output))
}

fn link_state(object: &Value) -> Option<&str> {
    object.pointer("/info/state").and_then(Value::as_str)
}

#[derive(Debug, Default)]
struct PipeWireNode {
    state: Option<String>,
    media_class: Option<String>,
    node_name: Option<String>,
    description: Option<String>,
    app_name: Option<String>,
}

impl PipeWireNode {
    fn from_object(object: &Value) -> Self {
        let props = object.pointer("/info/props");
        Self {
            state: object
                .pointer("/info/state")
                .and_then(Value::as_str)
                .map(str::to_string),
            media_class: props
                .and_then(|p| p.get("media.class"))
                .and_then(Value::as_str)
                .map(str::to_string),
            node_name: props
                .and_then(|p| p.get("node.name"))
                .and_then(Value::as_str)
                .map(str::to_string),
            description: props
                .and_then(|p| p.get("node.description"))
                .and_then(Value::as_str)
                .map(str::to_string),
            app_name: props
                .and_then(|p| p.get("application.name"))
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    fn is_screen_cast_source(&self) -> bool {
        if self.state.as_deref() == Some("suspended") {
            return false;
        }

        if !matches!(
            self.media_class.as_deref(),
            Some("Video/Source" | "Stream/Output/Video")
        ) {
            return false;
        }

        !self.looks_like_camera_source()
    }

    fn looks_like_camera_source(&self) -> bool {
        let haystack = [
            self.node_name.as_deref(),
            self.description.as_deref(),
            self.app_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ");

        haystack.contains("v4l2")
            || haystack.contains("camera")
            || haystack.contains("webcam")
            || haystack.contains("camlink")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn detects_active_non_camera_video_source_link() {
        let dump = json!([
            {
                "id": 10,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Video/Source",
                        "node.name": "portal-screen-cast"
                    }
                }
            },
            {
                "id": 20,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Stream/Input/Video",
                        "application.name": "Browser"
                    }
                }
            },
            {
                "id": 30,
                "type": "PipeWire:Interface:Link",
                "info": {
                    "state": "active",
                    "output-node-id": 10,
                    "input-node-id": 20
                }
            }
        ]);

        assert!(screen_sharing_from_pw_dump(&dump));
    }

    #[test]
    fn ignores_active_camera_video_source_link() {
        let dump = json!([
            {
                "id": 10,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Video/Source",
                        "node.name": "v4l2_input.usb-camera",
                        "node.description": "Integrated Camera"
                    }
                }
            },
            {
                "id": 20,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Stream/Input/Video",
                        "application.name": "Browser"
                    }
                }
            },
            {
                "id": 30,
                "type": "PipeWire:Interface:Link",
                "info": {
                    "state": "active",
                    "output-node-id": 10,
                    "input-node-id": 20
                }
            }
        ]);

        assert!(!screen_sharing_from_pw_dump(&dump));
    }

    #[test]
    fn detects_active_stream_output_video_link() {
        let dump = json!([
            {
                "id": 10,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Stream/Output/Video",
                        "node.name": "niri"
                    }
                }
            },
            {
                "id": 20,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Stream/Input/Video",
                        "node.name": "obs",
                        "media.type": "Video",
                        "media.category": "Capture"
                    }
                }
            },
            {
                "id": 30,
                "type": "PipeWire:Interface:Link",
                "info": {
                    "state": "active",
                    "output-node-id": 10,
                    "input-node-id": 20
                }
            }
        ]);

        assert!(screen_sharing_from_pw_dump(&dump));
    }
}
