//! Niri compositor backend using native socket IPC.
//!
//! This backend communicates with Niri via its Unix socket at $NIRI_SOCKET.
//! Protocol: JSON request/response, with event streaming support.
//!
//! Provides both workspace and window title functionality through a single
//! event stream connection.
//!
//! Reference: https://github.com/YaLTeR/niri/wiki/IPC

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use parking_lot::RwLock;
use serde_json::Value;
use tracing::{debug, error, trace, warn};

use super::{
    CompositorBackend, KeyboardLayoutCallback, KeyboardLayoutInfo, WindowCallback, WindowInfo,
    WorkspaceCallback, WorkspaceMeta, WorkspaceSnapshot,
};

const RECONNECT_INITIAL_MS: u64 = 1000;
const RECONNECT_MAX_MS: u64 = 30000;
const RECONNECT_MULTIPLIER: f64 = 1.5;

struct SharedState {
    workspace_snapshot: RwLock<WorkspaceSnapshot>,
    focused_window: RwLock<Option<WindowInfo>>,
    workspaces: RwLock<Vec<WorkspaceMeta>>,
    /// Map from Niri's u64 workspace ID to output name.
    id_to_output: RwLock<HashMap<u64, String>>,
    windows: RwLock<HashMap<u64, WindowData>>,
    /// Per-output active window info (output name -> WindowInfo).
    /// This tracks the "would be focused" window for each monitor.
    per_output_window: RwLock<HashMap<String, WindowInfo>>,
    /// Current keyboard layout info.
    keyboard_layout: RwLock<Option<KeyboardLayoutInfo>>,
    /// List of available keyboard layout names (from Niri's KeyboardLayouts).
    keyboard_layout_names: RwLock<Vec<String>>,
    /// Current keyboard layout index.
    keyboard_layout_idx: RwLock<usize>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            workspace_snapshot: RwLock::new(WorkspaceSnapshot::default()),
            focused_window: RwLock::new(None),
            workspaces: RwLock::new(Vec::new()),
            id_to_output: RwLock::new(HashMap::new()),
            windows: RwLock::new(HashMap::new()),
            per_output_window: RwLock::new(HashMap::new()),
            keyboard_layout: RwLock::new(None),
            keyboard_layout_names: RwLock::new(Vec::new()),
            keyboard_layout_idx: RwLock::new(0),
        }
    }
}

pub struct NiriBackend {
    #[allow(dead_code)] // For future filtering support
    allowed_outputs: Vec<String>,
    running: Arc<AtomicBool>,
    event_thread: Mutex<Option<JoinHandle<()>>>,
    socket_path: RwLock<Option<String>>,
    shared: Arc<SharedState>,
    callbacks: Mutex<Option<(WorkspaceCallback, WindowCallback)>>,
    keyboard_layout_callback: Mutex<Option<KeyboardLayoutCallback>>,
    window_list_callback: Mutex<Option<super::WindowListCallback>>,
}

#[derive(Debug, Clone)]
struct WindowData {
    id: u64,
    title: String,
    app_id: String,
    workspace_id: Option<u64>,
    is_focused: bool,
    is_urgent: bool,
    /// Column and tile position in the scrolling layout (niri-specific).
    /// Used for stable window-list ordering.
    layout_position: Option<(i32, i32)>,
}

#[derive(Clone, PartialEq, Eq)]
struct WindowSummary {
    id: u64,
    workspace_id: Option<u64>,
    title: String,
    app_id: String,
    is_focused: bool,
    is_urgent: bool,
    layout_position: Option<(i32, i32)>,
}

/// Extract `pos_in_scrolling_layout` from a niri window JSON value.
fn parse_layout_position(window: &Value) -> Option<(i32, i32)> {
    let layout = window.get("layout")?;
    let pos = layout.get("pos_in_scrolling_layout")?.as_array()?;
    if pos.len() >= 2 {
        Some((pos[0].as_i64()? as i32, pos[1].as_i64()? as i32))
    } else {
        None
    }
}

impl NiriBackend {
    pub fn new(outputs: Option<Vec<String>>) -> Self {
        Self {
            allowed_outputs: outputs.unwrap_or_default(),
            running: Arc::new(AtomicBool::new(false)),
            event_thread: Mutex::new(None),
            socket_path: RwLock::new(None),
            shared: Arc::new(SharedState::default()),
            callbacks: Mutex::new(None),
            keyboard_layout_callback: Mutex::new(None),
            window_list_callback: Mutex::new(None),
        }
    }

    /// Send a JSON request to Niri and get the response.
    fn send_request(&self, request: &Value) -> Option<Value> {
        let socket_path = self.socket_path.read();
        let socket_path = socket_path.as_ref()?;
        Self::send_request_static(socket_path, request)
    }

    /// Send a JSON request to Niri (static version for use without &self).
    fn send_request_static(socket_path: &str, request: &Value) -> Option<Value> {
        let mut stream = match UnixStream::connect(socket_path) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to connect to Niri socket: {}", e);
                return None;
            }
        };

        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

        let message = format!("{}\n", serde_json::to_string(request).ok()?);
        if let Err(e) = stream.write_all(message.as_bytes()) {
            error!("Failed to send request to Niri: {}", e);
            return None;
        }

        // Shutdown write side to signal end of request
        let _ = stream.shutdown(std::net::Shutdown::Write);

        let mut response = String::new();
        let mut reader = BufReader::new(stream);
        if let Err(e) = reader.read_line(&mut response) {
            error!("Failed to read Niri response: {}", e);
            return None;
        }

        match serde_json::from_str(&response) {
            Ok(v) => Some(v),
            Err(e) => {
                trace!("Failed to parse JSON from Niri: {}", e);
                None
            }
        }
    }

    fn get_windows_from_shared(shared: &Arc<SharedState>) -> Vec<super::Window> {
        let windows = shared.windows.read();
        let id_to_output = shared.id_to_output.read();
        let workspaces = shared.workspaces.read();

        // Build a map from workspace niri-ID to workspace display index for sorting.
        let ws_id_to_idx: HashMap<u64, i32> =
            workspaces.iter().map(|ws| (ws.id as u64, ws.idx)).collect();

        // Collect windows with their sorting keys.
        struct SortableWindow {
            window: super::Window,
            output_name: String,
            ws_idx: i32,
            layout_pos: (i32, i32),
        }

        let mut sortable: Vec<SortableWindow> = windows
            .values()
            .map(|win| {
                let output = win
                    .workspace_id
                    .and_then(|ws_id| id_to_output.get(&ws_id).cloned());

                let ws_idx = win
                    .workspace_id
                    .and_then(|ws_id| ws_id_to_idx.get(&ws_id).copied())
                    .unwrap_or(i32::MAX);

                let layout_pos = win.layout_position.unwrap_or((i32::MAX, i32::MAX));

                let output_name = output.clone().unwrap_or_default();

                SortableWindow {
                    window: super::Window {
                        id: win.id,
                        title: win.title.clone(),
                        app_id: win.app_id.clone(),
                        workspace_id: win.workspace_id.map(|id| id as i32),
                        output,
                        is_focused: win.is_focused,
                        is_urgent: win.is_urgent,
                    },
                    output_name,
                    ws_idx,
                    layout_pos,
                }
            })
            .collect();

        // Sort by output name, then workspace display index, then layout position,
        // then window ID. This mirrors the workspace sort order (output -> idx).
        sortable.sort_by(|a, b| {
            a.output_name
                .cmp(&b.output_name)
                .then(a.ws_idx.cmp(&b.ws_idx))
                .then(a.layout_pos.cmp(&b.layout_pos))
                .then(a.window.id.cmp(&b.window.id))
        });

        sortable.into_iter().map(|s| s.window).collect()
    }

    /// Process workspace list and update internal state.
    fn process_workspaces(shared: &SharedState, workspaces: &[Value]) {
        let mut ws_list = shared.workspaces.write();
        let mut id_to_output = shared.id_to_output.write();
        let mut snapshot = shared.workspace_snapshot.write();

        ws_list.clear();
        id_to_output.clear();
        snapshot.occupied_workspaces.clear();
        snapshot.urgent_workspaces.clear();
        snapshot.window_counts.clear();
        snapshot.active_workspace.clear();
        snapshot.per_output.clear();

        for ws in workspaces {
            let Some(ws_id) = ws.get("id").and_then(|v| v.as_u64()) else {
                continue;
            };
            let idx = ws.get("idx").and_then(|v| v.as_i64()).unwrap_or(1) as i32;
            // Use Niri's stable workspace ID for identity tracking.
            let stable_id = ws_id as i32;
            let name = ws
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| idx.to_string());

            // Get output name (Niri workspaces are per-monitor)
            let output = ws
                .get("output")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Store mapping from Niri workspace ID to output name
            if let Some(ref out) = output {
                id_to_output.insert(ws_id, out.clone());
            }
            ws_list.push(WorkspaceMeta {
                id: stable_id,
                idx,
                name,
                output: output.clone(),
            });

            // All workspaces in Niri are occupied (dynamic workspaces)
            snapshot.occupied_workspaces.insert(stable_id);
            // Initialize window count to 0, will be updated from window cache
            snapshot.window_counts.insert(stable_id, 0);

            let is_focused = ws
                .get("is_focused")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let is_active = ws
                .get("is_active")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if is_focused {
                snapshot.active_workspace.insert(stable_id);
            }

            if ws
                .get("is_urgent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                snapshot.urgent_workspaces.insert(stable_id);
            }

            // Build per-output state (Niri workspaces belong to specific outputs)
            if let Some(ref out_name) = output {
                let per_out = snapshot.per_output.entry(out_name.clone()).or_default();

                per_out.occupied_workspaces.insert(stable_id);
                // Window count will be updated from window cache
                per_out.window_counts.insert(stable_id, 0);

                // is_active means visible on this output, is_focused means globally focused
                if is_active {
                    per_out.active_workspace.insert(stable_id);
                }
            }
        }

        // Sort by output then positional index for consistent ordering
        ws_list.sort_by(|a, b| match (&a.output, &b.output) {
            (Some(oa), Some(ob)) => oa.cmp(ob).then(a.idx.cmp(&b.idx)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.idx.cmp(&b.idx),
        });

        // Update window counts from window cache
        // Must drop all write locks before calling update_window_counts
        drop(snapshot);
        drop(id_to_output);
        drop(ws_list);
        Self::update_window_counts(shared);
    }

    /// Update window counts from the window cache.
    fn update_window_counts(shared: &SharedState) {
        let win_cache = shared.windows.read();
        let id_to_output = shared.id_to_output.read();
        let mut snapshot = shared.workspace_snapshot.write();

        // Aggregate windows by workspace for progress calculation.
        // Ordering for progress only uses x to avoid y-based row jitter.
        let mut workspace_windows: HashMap<i32, Vec<(i32, u64, bool)>> = HashMap::new();

        // Reset global counts
        for count in snapshot.window_counts.values_mut() {
            *count = 0;
        }

        // Reset per-output counts
        for per_out in snapshot.per_output.values_mut() {
            for count in per_out.window_counts.values_mut() {
                *count = 0;
            }
        }

        // Reset computed progress; it will be repopulated from current cache state.
        snapshot.window_progress.clear();

        // Collect windows grouped by workspace and keep stable ordering information.
        for win in win_cache.values() {
            if let Some(ws_niri_id) = win.workspace_id {
                let stable_id = ws_niri_id as i32;
                let layout_pos = win.layout_position.unwrap_or((i32::MAX, i32::MAX));
                workspace_windows.entry(stable_id).or_default().push((
                    layout_pos.0,
                    win.id,
                    win.is_focused,
                ));
            }
        }

        // Compute per-workspace counts and progress from sorting order.
        for (stable_id, mut windows) in workspace_windows {
            windows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

            let total_windows = windows.len() as u32;
            let focused_index = windows.iter().position(|win| win.2).unwrap_or(0) as u32;

            *snapshot.window_counts.entry(stable_id).or_insert(0) = total_windows;

            snapshot.window_progress.insert(
                stable_id,
                super::WorkspaceWindowProgress {
                    focused_index,
                    total_windows,
                },
            );

            // Update per-output counts based on workspace ownership.
            if let Some(out_name) = id_to_output.get(&(stable_id as u64))
                && let Some(per_out) = snapshot.per_output.get_mut(out_name)
            {
                *per_out.window_counts.entry(stable_id).or_insert(0) = total_windows;
            }
        }

        // Ensure active workspaces are represented even when empty so the active
        // pill style still has progress metadata available.
        let active_workspaces: Vec<i32> = snapshot.active_workspace.iter().copied().collect();
        for ws_id in active_workspaces {
            snapshot
                .window_progress
                .entry(ws_id)
                .or_insert(super::WorkspaceWindowProgress {
                    focused_index: 0,
                    total_windows: 0,
                });
        }

        let per_output_active_workspaces: Vec<i32> = snapshot
            .per_output
            .values()
            .flat_map(|per_out| per_out.active_workspace.iter().copied())
            .collect();
        for ws_id in per_output_active_workspaces {
            snapshot
                .window_progress
                .entry(ws_id)
                .or_insert(super::WorkspaceWindowProgress {
                    focused_index: 0,
                    total_windows: 0,
                });
        }
    }

    /// Process window list and update internal state.
    fn process_windows(shared: &SharedState, windows: &[Value]) {
        let mut win_cache = shared.windows.write();
        let previous_focus_by_workspace: HashMap<i32, u64> = win_cache
            .values()
            .filter(|win| win.is_focused)
            .filter_map(|win| win.workspace_id.map(|ws_id| (ws_id as i32, win.id)))
            .collect();
        let mut parsed_windows = Vec::with_capacity(windows.len());
        let active_workspace_ids: HashSet<i32> = {
            let snapshot = shared.workspace_snapshot.read();
            let mut active: HashSet<i32> = snapshot.active_workspace.iter().copied().collect();
            for per_out in snapshot.per_output.values() {
                active.extend(per_out.active_workspace.iter().copied());
            }
            active
        };
        let mut focused_in_active_by_workspace: HashMap<i32, u64> = HashMap::new();

        for win in windows {
            let Some(win_id) = win.get("id").and_then(|v| v.as_u64()) else {
                continue;
            };

            let data = WindowData {
                id: win_id,
                title: win
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                app_id: win
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                workspace_id: win.get("workspace_id").and_then(|v| v.as_u64()),
                is_focused: win
                    .get("is_focused")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                is_urgent: win
                    .get("is_urgent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                layout_position: parse_layout_position(win),
            };

            if data.is_focused
                && data
                    .workspace_id
                    .is_some_and(|ws_id| active_workspace_ids.contains(&(ws_id as i32)))
                && let Some(ws_id) = data.workspace_id
            {
                focused_in_active_by_workspace.insert(ws_id as i32, win_id);
            }

            parsed_windows.push(data);
        }

        for mut parsed in parsed_windows {
            if let Some(ws_id) = parsed.workspace_id.map(|ws| ws as i32) {
                parsed.is_focused = focused_in_active_by_workspace
                    .get(&ws_id)
                    .copied()
                    .is_some_and(|focused_id| focused_id == parsed.id)
                    || previous_focus_by_workspace
                        .get(&ws_id)
                        .is_some_and(|focused_id| *focused_id == parsed.id);
            } else {
                parsed.is_focused = false;
            }
            win_cache.insert(parsed.id, parsed);
        }

        drop(win_cache);
        Self::update_window_counts(shared);
        Self::update_focused_window_from_cache(shared);
        Self::update_per_output_windows(shared);
    }

    fn summarize_windows(shared: &SharedState) -> Vec<WindowSummary> {
        let mut windows = shared
            .windows
            .read()
            .values()
            .map(|win| WindowSummary {
                id: win.id,
                workspace_id: win.workspace_id,
                title: win.title.clone(),
                app_id: win.app_id.clone(),
                is_focused: win.is_focused,
                is_urgent: win.is_urgent,
                layout_position: win.layout_position,
            })
            .collect::<Vec<_>>();
        windows.sort_by_key(|item| item.id);
        windows
    }

    /// Set a single focused window in the cache and clear stale focus state.
    fn set_focused_window(shared: &SharedState, focused_window_id: Option<u64>) {
        let mut win_cache = shared.windows.write();
        for win in win_cache.values_mut() {
            win.is_focused = focused_window_id.is_some_and(|id| win.id == id);
        }
    }

    /// Update focus state for a single workspace only.
    fn set_focused_window_in_workspace(
        shared: &SharedState,
        ws_niri_id: u64,
        focused_window_id: Option<u64>,
    ) {
        let mut win_cache = shared.windows.write();
        for win in win_cache.values_mut() {
            if win.workspace_id == Some(ws_niri_id) {
                win.is_focused = focused_window_id == Some(win.id);
            }
        }
    }

    /// Update per-output active window info from window cache and workspace state.
    fn update_per_output_windows(shared: &SharedState) {
        let win_cache = shared.windows.read();
        let id_to_output = shared.id_to_output.read();
        let snapshot = shared.workspace_snapshot.read();
        let mut per_output = shared.per_output_window.write();

        // For each output, find the window to display on its active workspace
        for (out_name, per_out) in &snapshot.per_output {
            // Find active workspace's niri ID for this output
            let active_ws_id = id_to_output.iter().find_map(|(&ws_id, out)| {
                if out == out_name {
                    let stable_id = ws_id as i32;
                    per_out
                        .active_workspace
                        .contains(&stable_id)
                        .then_some(ws_id)
                } else {
                    None
                }
            });

            // Find best window on that workspace (prefer focused)
            let win_info = active_ws_id.and_then(|ws_id| {
                let mut best: Option<&WindowData> = None;
                for win in win_cache.values() {
                    if win.workspace_id == Some(ws_id) {
                        if win.is_focused {
                            return Some(win);
                        }
                        best = best.or(Some(win));
                    }
                }
                best
            });

            let info = win_info
                .map(|win| WindowInfo {
                    title: win.title.clone(),
                    app_id: win.app_id.clone(),
                    workspace_id: active_ws_id.map(|id| id as i32),
                    output: Some(out_name.clone()),
                })
                .unwrap_or_else(|| WindowInfo {
                    output: Some(out_name.clone()),
                    ..Default::default()
                });

            per_output.insert(out_name.clone(), info);
        }
    }

    /// Update focused window info from window cache.
    fn update_focused_window_from_cache(shared: &SharedState) -> bool {
        let win_cache = shared.windows.read();
        let id_to_output = shared.id_to_output.read();

        let mut new_focused: Option<WindowInfo> = None;

        for win in win_cache.values() {
            if !win.is_focused {
                continue;
            }

            let workspace_id = win.workspace_id.map(|ws_id| ws_id as i32);
            // Look up the output directly from Niri's workspace ID
            let output = win
                .workspace_id
                .and_then(|ws_id| id_to_output.get(&ws_id).cloned());

            new_focused = Some(WindowInfo {
                title: win.title.clone(),
                app_id: win.app_id.clone(),
                workspace_id,
                output,
            });
            break;
        }

        let mut focused = shared.focused_window.write();
        let changed = *focused != new_focused;
        *focused = new_focused;
        changed
    }

    /// Update a single window in the cache.
    ///
    /// Returns true if this changes the cached window data.
    fn update_single_window(shared: &SharedState, window: &Value) -> bool {
        let Some(win_id) = window.get("id").and_then(|v| v.as_u64()) else {
            return false;
        };

        let title = window
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let app_id = window
            .get("app_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let workspace_id = window.get("workspace_id").and_then(|v| v.as_u64());
        let is_focused = window
            .get("is_focused")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let is_urgent = window
            .get("is_urgent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let data = WindowData {
            id: win_id,
            title,
            app_id,
            workspace_id,
            is_focused,
            is_urgent,
            layout_position: parse_layout_position(window),
        };
        let active_workspace_ids: HashSet<i32> = {
            let snapshot = shared.workspace_snapshot.read();
            let mut active: HashSet<i32> = snapshot.active_workspace.iter().copied().collect();
            for per_out in snapshot.per_output.values() {
                active.extend(per_out.active_workspace.iter().copied());
            }
            active
        };
        let is_focused = data
            .workspace_id
            .is_some_and(|ws_id| active_workspace_ids.contains(&(ws_id as i32)))
            && is_focused;
        let workspace_id = data.workspace_id;

        let mut changed = false;

        {
            let mut win_cache = shared.windows.write();
            if let Some(existing) = win_cache.get_mut(&win_id) {
                if existing.title != data.title {
                    existing.title = data.title.clone();
                    changed = true;
                }
                if existing.app_id != data.app_id {
                    existing.app_id = data.app_id.clone();
                    changed = true;
                }
                if existing.workspace_id != data.workspace_id {
                    existing.workspace_id = data.workspace_id;
                    changed = true;
                }
                if existing.is_urgent != data.is_urgent {
                    existing.is_urgent = data.is_urgent;
                    changed = true;
                }
                if existing.layout_position != data.layout_position {
                    existing.layout_position = data.layout_position;
                    changed = true;
                }
                if existing.is_focused != is_focused {
                    existing.is_focused = is_focused;
                    changed = true;
                }
            } else {
                changed = true;
                let mut new_data = data;
                new_data.is_focused = is_focused;
                win_cache.insert(win_id, new_data);
            }
        }

        if !changed {
            return false;
        }

        if is_focused && let Some(ws_id) = workspace_id {
            let mut win_cache = shared.windows.write();
            for win in win_cache.values_mut() {
                if win.workspace_id == Some(ws_id) {
                    win.is_focused = win.id == win_id;
                }
            }
        }
        true
    }

    /// Fetch initial state from Niri.
    fn fetch_initial_state(socket_path: &str, shared: &SharedState) {
        // Fetch workspaces
        if let Some(reply) =
            Self::send_request_static(socket_path, &Value::String("Workspaces".to_string()))
            && let Some(ok) = reply.get("Ok")
            && let Some(workspaces) = ok.get("Workspaces").and_then(|v| v.as_array())
        {
            Self::process_workspaces(shared, workspaces);
        }

        // Fetch windows
        if let Some(reply) =
            Self::send_request_static(socket_path, &Value::String("Windows".to_string()))
            && let Some(ok) = reply.get("Ok")
            && let Some(windows) = ok.get("Windows").and_then(|v| v.as_array())
        {
            Self::process_windows(shared, windows);
        }

        // Fetch keyboard layouts
        Self::fetch_keyboard_layouts(socket_path, shared);

        debug!("Fetched initial Niri state");
    }

    /// Fetch keyboard layouts from Niri.
    fn fetch_keyboard_layouts(socket_path: &str, shared: &SharedState) {
        let Some(reply) =
            Self::send_request_static(socket_path, &Value::String("KeyboardLayouts".to_string()))
        else {
            debug!("fetch_keyboard_layouts: failed to query from Niri");
            return;
        };

        let Some(ok) = reply.get("Ok") else {
            debug!("fetch_keyboard_layouts: Niri returned error: {:?}", reply);
            return;
        };

        let Some(kb_layouts) = ok.get("KeyboardLayouts") else {
            debug!("fetch_keyboard_layouts: no KeyboardLayouts in response");
            return;
        };

        Self::process_keyboard_layouts(shared, kb_layouts);
    }

    /// Process keyboard layout data from Niri.
    fn process_keyboard_layouts(shared: &SharedState, kb_layouts: &Value) -> bool {
        let names = kb_layouts
            .get("names")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|entry| entry.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let current_idx = kb_layouts
            .get("current_idx")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let (before_names, before_idx) = {
            let before_names = shared.keyboard_layout_names.read();
            let before_idx = *shared.keyboard_layout_idx.read();
            (before_names.clone(), before_idx)
        };

        if before_names.as_slice() == names.as_slice() && before_idx == current_idx {
            return false;
        }

        let layout_count = names.len();
        let layout_name = names.get(current_idx).cloned().unwrap_or_default();

        debug!(
            "process_keyboard_layouts: idx={}, layout='{}', count={}",
            current_idx, layout_name, layout_count
        );

        *shared.keyboard_layout_names.write() = names;
        *shared.keyboard_layout_idx.write() = current_idx;
        *shared.keyboard_layout.write() = Some(KeyboardLayoutInfo {
            layout_name,
            short_name: String::new(),

            layout_count: Some(layout_count),
        });

        true
    }

    /// Process a keyboard layout switch event.
    fn process_keyboard_layout_switch(shared: &SharedState, idx: usize) -> bool {
        let (layout_name, layout_count, current_idx) = {
            let names = shared.keyboard_layout_names.read();
            let layout_name = names.get(idx).cloned().unwrap_or_default();
            let layout_count = names.len();
            let current_idx = *shared.keyboard_layout_idx.read();
            (layout_name, layout_count, current_idx)
        };

        if idx == current_idx {
            return false;
        }

        debug!(
            "process_keyboard_layout_switch: idx={}, layout='{}'",
            idx, layout_name
        );

        *shared.keyboard_layout_idx.write() = idx;
        *shared.keyboard_layout.write() = Some(KeyboardLayoutInfo {
            layout_name,
            short_name: String::new(),

            layout_count: Some(layout_count),
        });

        true
    }

    /// Handle a Niri event.
    ///
    /// Returns (workspace_changed, window_changed, keyboard_layout_changed).
    fn handle_event(shared: &SharedState, event: &Value) -> (bool, bool, bool) {
        let mut workspace_changed = false;
        let mut window_changed = false;
        let mut keyboard_layout_changed = false;

        if let Some(kb_layouts_changed) = event.get("KeyboardLayoutsChanged") {
            // Full layout list changed (e.g., user reconfigured layouts)
            if let Some(kb_layouts) = kb_layouts_changed.get("keyboard_layouts") {
                keyboard_layout_changed = Self::process_keyboard_layouts(shared, kb_layouts);
            }
        } else if let Some(kb_switched) = event.get("KeyboardLayoutSwitched") {
            // Just switched to a different layout by index
            if let Some(idx) = kb_switched.get("idx").and_then(|v| v.as_u64()) {
                keyboard_layout_changed =
                    Self::process_keyboard_layout_switch(shared, idx as usize);
            }
        } else if let Some(workspaces_changed) = event.get("WorkspacesChanged") {
            if let Some(workspaces) = workspaces_changed
                .get("workspaces")
                .and_then(|v| v.as_array())
            {
                let before = shared.workspace_snapshot.read().clone();
                Self::process_workspaces(shared, workspaces);
                let after = shared.workspace_snapshot.read().clone();
                workspace_changed = before != after;
            }
        } else if let Some(workspace_activated) = event.get("WorkspaceActivated") {
            let ws_niri_id = workspace_activated.get("id").and_then(|v| v.as_u64());
            let is_focused = workspace_activated
                .get("focused")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if let Some(ws_id) = ws_niri_id {
                let stable_id = ws_id as i32;
                let mut switched_workspace = false;
                let id_to_output = shared.id_to_output.read();
                let output = id_to_output.get(&ws_id).cloned();
                drop(id_to_output);

                let mut snapshot = shared.workspace_snapshot.write();
                let mut output_changed = false;

                if is_focused && !snapshot.active_workspace.contains(&stable_id) {
                    snapshot.active_workspace.clear();
                    snapshot.active_workspace.insert(stable_id);
                    switched_workspace = true;
                }

                if let Some(ref out_name) = output
                    && let Some(per_out) = snapshot.per_output.get_mut(out_name)
                    && !per_out.active_workspace.contains(&stable_id)
                {
                    per_out.active_workspace.clear();
                    per_out.active_workspace.insert(stable_id);
                    output_changed = true;
                }

                drop(snapshot);

                if switched_workspace || output_changed {
                    Self::update_window_counts(shared);
                    Self::update_per_output_windows(shared);
                    window_changed = true;
                    workspace_changed = true;
                }
            }
        } else if let Some(urgency_changed) = event.get("WorkspaceUrgencyChanged") {
            if let Some(ws_id) = urgency_changed.get("id").and_then(|v| v.as_u64()) {
                let is_urgent = urgency_changed
                    .get("urgent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let stable_id = ws_id as i32;
                let contains_workspace = {
                    let snapshot = shared.workspace_snapshot.read();
                    snapshot.occupied_workspaces.contains(&stable_id)
                };
                if contains_workspace {
                    let mut snapshot = shared.workspace_snapshot.write();
                    if is_urgent {
                        workspace_changed = snapshot.urgent_workspaces.insert(stable_id);
                    } else {
                        workspace_changed = snapshot.urgent_workspaces.remove(&stable_id);
                    }
                }
            }
        } else if let Some(windows_changed) = event.get("WindowsChanged") {
            if let Some(windows) = windows_changed.get("windows").and_then(|v| v.as_array()) {
                let has_malformed_window = windows
                    .iter()
                    .any(|window| window.get("id").and_then(|v| v.as_u64()).is_none());
                let before_snapshot = shared.workspace_snapshot.read().clone();
                let before_windows = Self::summarize_windows(shared);
                Self::process_windows(shared, windows);
                let after_snapshot = shared.workspace_snapshot.read().clone();
                let after_windows = Self::summarize_windows(shared);
                workspace_changed = before_snapshot != after_snapshot || has_malformed_window;
                window_changed = before_windows != after_windows || has_malformed_window;
                workspace_changed |= window_changed;
            }
        } else if let Some(window_opened) = event.get("WindowOpenedOrChanged") {
            if let Some(window) = window_opened.get("window") {
                let before_windows = Self::summarize_windows(shared);
                let changed = Self::update_single_window(shared, window);
                if !changed {
                    return (workspace_changed, window_changed, keyboard_layout_changed);
                }

                Self::update_window_counts(shared);
                if let Some(ws_id) = window.get("workspace_id").and_then(|v| v.as_u64()) {
                    let stable_id = ws_id as i32;
                    let mut snapshot = shared.workspace_snapshot.write();
                    snapshot.occupied_workspaces.insert(stable_id);
                }
                Self::update_focused_window_from_cache(shared);
                Self::update_per_output_windows(shared);

                let after_windows = Self::summarize_windows(shared);
                window_changed = before_windows != after_windows;
                workspace_changed = true;
            }
        } else if let Some(layouts_changed) = event.get("WindowLayoutsChanged") {
            // changes is Vec<(u64, WindowLayout)> which serializes as an array of tuples:
            // [[window_id, {layout_obj}], ...]
            if let Some(changes) = layouts_changed.get("changes").and_then(|v| v.as_array()) {
                let mut win_cache = shared.windows.write();
                let mut changed = false;

                for entry in changes {
                    let entry = match entry.as_array() {
                        Some(arr) if arr.len() >= 2 => arr,
                        _ => continue,
                    };
                    let win_id = match entry[0].as_u64() {
                        Some(id) => id,
                        None => continue,
                    };
                    if let Some(win) = win_cache.get_mut(&win_id) {
                        // entry[1] is a WindowLayout object with pos_in_scrolling_layout directly
                        let new_layout_position = entry[1]
                            .get("pos_in_scrolling_layout")
                            .and_then(|v| v.as_array())
                            .and_then(|arr| {
                                if arr.len() >= 2 {
                                    Some((arr[0].as_i64()? as i32, arr[1].as_i64()? as i32))
                                } else {
                                    None
                                }
                            });
                        if let Some(new_position) = new_layout_position
                            && win.layout_position != Some(new_position)
                        {
                            win.layout_position = Some(new_position);
                            changed = true;
                        }
                    }
                }
                drop(win_cache);
                if changed {
                    Self::update_window_counts(shared);
                    window_changed = true;
                    workspace_changed = true;
                }
            }
        } else if let Some(window_closed) = event.get("WindowClosed") {
            if let Some(win_id) = window_closed.get("id").and_then(|v| v.as_u64()) {
                let removed = shared.windows.write().remove(&win_id);
                if removed.is_some() {
                    Self::update_window_counts(shared);
                    Self::update_focused_window_from_cache(shared);
                    Self::update_per_output_windows(shared);
                    window_changed = true;
                    workspace_changed = true;
                }
            }
        } else if let Some(focus_changed) = event.get("WindowFocusChanged") {
            let win_id = focus_changed.get("id").and_then(|v| v.as_u64());
            let Some(win_id) = win_id else {
                Self::set_focused_window(shared, None);
                Self::update_window_counts(shared);
                Self::update_focused_window_from_cache(shared);
                Self::update_per_output_windows(shared);
                window_changed = true;
                workspace_changed = true;
                return (workspace_changed, window_changed, keyboard_layout_changed);
            };

            let workspace_id = {
                let win_cache = shared.windows.read();
                win_cache.get(&win_id).and_then(|win| win.workspace_id)
            };

            if let Some(ws_id) = workspace_id {
                let is_workspace_active = {
                    let snapshot = shared.workspace_snapshot.read();
                    let stable_id = ws_id as i32;
                    snapshot.active_workspace.contains(&stable_id)
                        || snapshot
                            .per_output
                            .values()
                            .any(|per_out| per_out.active_workspace.contains(&stable_id))
                };
                if !is_workspace_active {
                    return (workspace_changed, window_changed, keyboard_layout_changed);
                }
            } else {
                return (workspace_changed, window_changed, keyboard_layout_changed);
            }

            if let Some(ws_id) = workspace_id {
                Self::set_focused_window_in_workspace(shared, ws_id, Some(win_id));
            }
            Self::update_window_counts(shared);
            Self::update_focused_window_from_cache(shared);
            Self::update_per_output_windows(shared);
            window_changed = true;
            workspace_changed = true;
        } else if let Some(urgency_changed) = event.get("WindowUrgencyChanged") {
            let win_id = urgency_changed.get("id").and_then(|v| v.as_u64());
            let is_urgent = urgency_changed
                .get("urgent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if let Some(win_id) = win_id {
                let mut win_cache = shared.windows.write();
                if let Some(win) = win_cache.get_mut(&win_id)
                    && win.is_urgent != is_urgent
                {
                    win.is_urgent = is_urgent;
                    window_changed = true;
                }
            }
        } else if let Some(active_changed) = event.get("WorkspaceActiveWindowChanged") {
            let ws_niri_id = active_changed.get("workspace_id").and_then(|v| v.as_u64());
            let active_win_id = active_changed
                .get("active_window_id")
                .and_then(|v| v.as_u64());

            if let Some(ws_id) = ws_niri_id {
                let before_active_win_id = {
                    let win_cache = shared.windows.read();
                    win_cache
                        .values()
                        .find(|win| win.workspace_id == Some(ws_id) && win.is_focused)
                        .map(|win| win.id)
                };
                if before_active_win_id == active_win_id {
                    return (workspace_changed, window_changed, keyboard_layout_changed);
                }

                let id_to_output = shared.id_to_output.read();

                if let Some(output) = id_to_output.get(&ws_id).cloned() {
                    let workspace_id = ws_id as i32;
                    drop(id_to_output);

                    let is_workspace_active = {
                        let snapshot = shared.workspace_snapshot.read();
                        snapshot.active_workspace.contains(&workspace_id)
                            || snapshot
                                .per_output
                                .values()
                                .any(|per_out| per_out.active_workspace.contains(&workspace_id))
                    };

                    if !is_workspace_active {
                        return (workspace_changed, window_changed, keyboard_layout_changed);
                    }

                    if let Some(active_win_id) = active_win_id {
                        Self::set_focused_window_in_workspace(shared, ws_id, Some(active_win_id));
                    } else {
                        Self::set_focused_window_in_workspace(shared, ws_id, None);
                    }
                    Self::update_window_counts(shared);
                    Self::update_focused_window_from_cache(shared);

                    let win_info = if let Some(win_id) = active_win_id {
                        let win_cache = shared.windows.read();
                        win_cache.get(&win_id).map(|win| WindowInfo {
                            title: win.title.clone(),
                            app_id: win.app_id.clone(),
                            workspace_id: Some(workspace_id),
                            output: Some(output.clone()),
                        })
                    } else {
                        None
                    };

                    let mut per_output = shared.per_output_window.write();
                    per_output.insert(
                        output.clone(),
                        win_info.unwrap_or(WindowInfo {
                            output: Some(output),
                            ..Default::default()
                        }),
                    );
                    window_changed = true;
                    workspace_changed = true;
                }
            }
        }

        (workspace_changed, window_changed, keyboard_layout_changed)
    }

    /// Run the event loop (in background thread).
    fn event_loop(
        running: Arc<AtomicBool>,
        shared: Arc<SharedState>,
        socket_path: String,
        callbacks: Option<(WorkspaceCallback, WindowCallback)>,
        kb_callback: Option<KeyboardLayoutCallback>,
        window_list_callback: Option<super::WindowListCallback>,
    ) {
        // Fetch initial state
        Self::fetch_initial_state(&socket_path, &shared);

        // Emit initial state
        if let Some((ref ws_cb, ref win_cb)) = callbacks {
            ws_cb(shared.workspace_snapshot.read().clone());
            // Emit window info for all outputs (including empty info for outputs with no active window)
            let per_output = shared.per_output_window.read();
            for win_info in per_output.values() {
                win_cb(win_info.clone());
            }
        }

        // Emit the full initial window list for consumers that register for it.
        if let Some(ref wl_cb) = window_list_callback {
            let windows = Self::get_windows_from_shared(&shared);
            wl_cb(super::WindowListSnapshot { windows });
        }

        // Emit initial keyboard layout
        if let Some(ref kb_cb) = kb_callback
            && let Some(ref info) = *shared.keyboard_layout.read()
        {
            kb_cb(info.clone());
        }

        // Exponential backoff state
        let mut backoff_ms = RECONNECT_INITIAL_MS;

        while running.load(Ordering::SeqCst) {
            // Connect and request event stream
            let stream = match UnixStream::connect(&socket_path) {
                Ok(s) => {
                    // Reset backoff on successful connection
                    backoff_ms = RECONNECT_INITIAL_MS;
                    s
                }
                Err(e) => {
                    if running.load(Ordering::SeqCst) {
                        warn!(
                            "Failed to connect to Niri socket: {}. Retrying in {}ms",
                            e, backoff_ms
                        );
                        thread::sleep(Duration::from_millis(backoff_ms));
                        // Exponential backoff with cap
                        backoff_ms = ((backoff_ms as f64) * RECONNECT_MULTIPLIER)
                            .min(RECONNECT_MAX_MS as f64)
                            as u64;
                    }
                    continue;
                }
            };

            // Request event stream
            let message = "\"EventStream\"\n";
            if stream
                .try_clone()
                .ok()
                .and_then(|mut s| s.write_all(message.as_bytes()).ok())
                .is_none()
            {
                if running.load(Ordering::SeqCst) {
                    warn!(
                        "Failed to request Niri event stream. Retrying in {}ms",
                        backoff_ms
                    );
                    thread::sleep(Duration::from_millis(backoff_ms));
                    // Exponential backoff with cap
                    backoff_ms = ((backoff_ms as f64) * RECONNECT_MULTIPLIER)
                        .min(RECONNECT_MAX_MS as f64) as u64;
                }
                continue;
            }

            // Set read timeout for graceful shutdown
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));

            let reader = BufReader::new(stream);

            for line in reader.lines() {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                match line {
                    Ok(line) => {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }

                        match serde_json::from_str::<Value>(line) {
                            Ok(event) => {
                                // Skip "Ok": "Handled" responses
                                if event.get("Ok").and_then(|v| v.as_str()) == Some("Handled") {
                                    continue;
                                }

                                let (ws_changed, win_changed, kb_changed) =
                                    Self::handle_event(&shared, &event);

                                if let Some((ref ws_cb, ref win_cb)) = callbacks {
                                    if ws_changed {
                                        ws_cb(shared.workspace_snapshot.read().clone());
                                    }
                                    if win_changed {
                                        // Emit updates for all outputs with their current active window
                                        let per_output = shared.per_output_window.read();
                                        for win_info in per_output.values() {
                                            win_cb(win_info.clone());
                                        }
                                    }
                                }

                                if kb_changed
                                    && let Some(ref kb_cb) = kb_callback
                                    && let Some(ref info) = *shared.keyboard_layout.read()
                                {
                                    kb_cb(info.clone());
                                }

                                if (win_changed || ws_changed)
                                    && let Some(ref wl_cb) = window_list_callback
                                {
                                    let windows = Self::get_windows_from_shared(&shared);
                                    wl_cb(super::WindowListSnapshot { windows });
                                }
                            }
                            Err(e) => {
                                trace!("Failed to parse Niri event: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        // Timeout is expected
                        if e.kind() != std::io::ErrorKind::WouldBlock
                            && e.kind() != std::io::ErrorKind::TimedOut
                        {
                            if running.load(Ordering::SeqCst) {
                                error!("Error reading from Niri socket: {}", e);
                            }
                            break;
                        }
                    }
                }
            }
        }

        debug!("Niri event loop exiting");
    }
}

impl CompositorBackend for NiriBackend {
    fn start(&self, on_workspace_update: WorkspaceCallback, on_window_update: WindowCallback) {
        if self.running.swap(true, Ordering::SeqCst) {
            warn!("NiriBackend already running");
            return;
        }

        debug!("Starting NiriBackend");

        // Get socket path from environment and store on `self` FIRST
        // This ensures socket_path is set for switch_workspace()
        let socket_path = match env::var("NIRI_SOCKET") {
            Ok(p) => p,
            Err(_) => {
                warn!("NIRI_SOCKET not set");
                self.running.store(false, Ordering::SeqCst);
                return;
            }
        };
        *self.socket_path.write() = Some(socket_path.clone());

        // Store callbacks for potential later use
        *self.callbacks.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((on_workspace_update.clone(), on_window_update.clone()));

        // Clone shared state and running flag for the thread
        let running = Arc::clone(&self.running);
        let shared = Arc::clone(&self.shared);
        let callbacks = Some((on_workspace_update, on_window_update));
        let kb_callback = self
            .keyboard_layout_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let window_list_callback = self
            .window_list_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        // Start event loop thread
        let handle = thread::Builder::new()
            .name("niri-event-loop".into())
            .spawn(move || {
                Self::event_loop(
                    running,
                    shared,
                    socket_path,
                    callbacks,
                    kb_callback,
                    window_list_callback,
                );
            })
            .ok();

        *self.event_thread.lock().unwrap_or_else(|e| e.into_inner()) = handle;

        debug!("NiriBackend started");
    }

    fn stop(&self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }

        debug!("Stopping NiriBackend");

        if let Some(handle) = self
            .event_thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = handle.join();
        }

        debug!("NiriBackend stopped");
    }

    fn list_workspaces(&self) -> Vec<WorkspaceMeta> {
        let workspaces = self.shared.workspaces.read();
        if workspaces.is_empty() {
            // Return default workspaces if not initialized yet
            (1..=10)
                .map(|i| WorkspaceMeta {
                    id: i,
                    idx: i,
                    name: i.to_string(),
                    output: None,
                })
                .collect()
        } else {
            workspaces.clone()
        }
    }

    fn get_workspace_snapshot(&self) -> WorkspaceSnapshot {
        // If not initialized, try to fetch state
        let socket_path = self.socket_path.read();
        if socket_path.is_none()
            && let Ok(path) = env::var("NIRI_SOCKET")
        {
            drop(socket_path);
            *self.socket_path.write() = Some(path.clone());
            Self::fetch_initial_state(&path, &self.shared);
        }
        self.shared.workspace_snapshot.read().clone()
    }

    fn get_focused_window(&self) -> Option<WindowInfo> {
        self.shared.focused_window.read().clone()
    }

    fn switch_workspace(&self, workspace_id: i32) {
        // Use stable workspace ID (not positional index) for reliable switching.
        let request = serde_json::json!({
            "Action": {
                "FocusWorkspace": {
                    "reference": {
                        "Id": workspace_id
                    }
                }
            }
        });
        let _ = self.send_request(&request);
    }

    fn quit_compositor(&self) {
        debug!("Sending quit request to Niri");
        let request = serde_json::json!({
            "Action": {
                "Quit": {
                    "skip_confirmation": true
                }
            }
        });
        let _ = self.send_request(&request);
    }

    fn name(&self) -> &'static str {
        "Niri"
    }

    fn set_keyboard_layout_callback(&self, callback: KeyboardLayoutCallback) {
        *self
            .keyboard_layout_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(callback);
    }

    fn set_window_list_callback(&self, callback: super::WindowListCallback) {
        *self
            .window_list_callback
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(callback);
    }

    fn get_keyboard_layout(&self) -> Option<KeyboardLayoutInfo> {
        self.shared.keyboard_layout.read().clone()
    }

    fn switch_keyboard_layout_next(&self) {
        let request = serde_json::json!({
            "Action": {
                "SwitchLayout": {
                    "layout": "Next"
                }
            }
        });
        let _ = self.send_request(&request);
    }

    fn list_windows(&self) -> Vec<super::Window> {
        Self::get_windows_from_shared(&self.shared)
    }

    fn focus_window(&self, window_id: u64) {
        let request = serde_json::json!({
            "Action": {
                "FocusWindow": {
                    "id": window_id
                }
            }
        });
        let _ = self.send_request(&request);
    }
}

impl Drop for NiriBackend {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::compositor::WindowListSnapshot;
    use serde_json::json;
    use std::sync::Arc;

    fn event_snapshot_changed(shared: &Arc<SharedState>, event: Value) -> (bool, bool, bool) {
        NiriBackend::handle_event(shared, &event)
    }

    fn active_fraction(shared: &Arc<SharedState>, workspace_id: i32) -> Option<f64> {
        let snapshot = shared.workspace_snapshot.read();
        if !snapshot.active_workspace.contains(&workspace_id) {
            return None;
        }

        snapshot
            .window_progress
            .get(&workspace_id)
            .and_then(|progress| progress.fraction())
    }

    fn window_progress_fraction(shared: &Arc<SharedState>, workspace_id: i32) -> Option<f64> {
        let snapshot = shared.workspace_snapshot.read();
        snapshot
            .window_progress
            .get(&workspace_id)
            .and_then(|progress| progress.fraction())
    }

    fn run_event_with_simulated_callbacks(
        shared: &Arc<SharedState>,
        event: Value,
    ) -> (
        Vec<super::WorkspaceSnapshot>,
        Vec<WindowInfo>,
        Vec<WindowListSnapshot>,
        Vec<String>,
    ) {
        let mut workspace_updates = Vec::new();
        let mut window_updates = Vec::new();
        let mut window_list_updates = Vec::new();
        let mut callback_order = Vec::new();

        let (workspace_changed, window_changed, _) = event_snapshot_changed(shared, event);

        if workspace_changed {
            callback_order.push("workspace".to_string());
            workspace_updates.push(shared.workspace_snapshot.read().clone());
        }
        if window_changed {
            callback_order.push("window".to_string());
            let per_output = shared.per_output_window.read();
            for win_info in per_output.values() {
                window_updates.push(win_info.clone());
            }
        }
        if workspace_changed || window_changed {
            callback_order.push("window_list".to_string());
            let windows = NiriBackend::get_windows_from_shared(shared);
            window_list_updates.push(WindowListSnapshot { windows });
        }

        (
            workspace_updates,
            window_updates,
            window_list_updates,
            callback_order,
        )
    }

    fn workspaces_payload() -> Value {
        json!({
            "WorkspacesChanged": {
                "workspaces": [
                    {
                        "id": 1,
                        "idx": 1,
                        "name": "1",
                        "output": "eDP-1",
                        "is_focused": true,
                        "is_active": true,
                    },
                    {
                        "id": 2,
                        "idx": 2,
                        "name": "2",
                        "output": "eDP-1",
                        "is_focused": false,
                        "is_active": false,
                    },
                ]
            }
        })
    }

    fn windows_payload() -> Value {
        json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "ws1-left",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 102,
                        "title": "ws1-mid",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "id": 103,
                        "title": "ws1-right",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [3, 1]
                        }
                    },
                    {
                        "id": 201,
                        "title": "ws2-only",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                ]
            }
        })
    }

    fn windows_payload_multi_ws2() -> Value {
        json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "ws1-left",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 102,
                        "title": "ws1-mid",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "id": 103,
                        "title": "ws1-right",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [3, 1]
                        }
                    },
                    {
                        "id": 201,
                        "title": "ws2-left",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 202,
                        "title": "ws2-right",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                ]
            }
        })
    }

    fn workspaces_payload_ws2_and_ws3() -> Value {
        json!({
            "WorkspacesChanged": {
                "workspaces": [
                    {
                        "id": 2,
                        "idx": 2,
                        "name": "2",
                        "output": "eDP-1",
                        "is_focused": true,
                        "is_active": true,
                    },
                    {
                        "id": 3,
                        "idx": 3,
                        "name": "3",
                        "output": "eDP-1",
                        "is_focused": false,
                        "is_active": false,
                    },
                ]
            }
        })
    }

    fn windows_payload_ws2_three_ws3_one() -> Value {
        json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 201,
                        "title": "ws2-left",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 202,
                        "title": "ws2-mid",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "id": 203,
                        "title": "ws2-right",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [3, 1]
                        }
                    },
                    {
                        "id": 301,
                        "title": "ws3-only",
                        "app_id": "a",
                        "workspace_id": 3,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                ]
            }
        })
    }

    fn windows_payload_ws3_only() -> Value {
        json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 301,
                        "title": "ws3-only",
                        "app_id": "a",
                        "workspace_id": 3,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                ]
            }
        })
    }

    fn workspaces_payload_dual_outputs() -> Value {
        json!({
            "WorkspacesChanged": {
                "workspaces": [
                    {
                        "id": 1,
                        "idx": 1,
                        "name": "1",
                        "output": "eDP-1",
                        "is_focused": true,
                        "is_active": true,
                    },
                    {
                        "id": 2,
                        "idx": 2,
                        "name": "2",
                        "output": "HDMI-A-1",
                        "is_focused": false,
                        "is_active": false,
                    },
                ]
            }
        })
    }

    fn windows_payload_dual_outputs() -> Value {
        json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "ws1-left",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 102,
                        "title": "ws1-right",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "id": 201,
                        "title": "ws2-left",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 202,
                        "title": "ws2-right",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                ]
            }
        })
    }

    fn workspaces_payload_dual_outputs_ws2_focused_only() -> Value {
        json!({
            "WorkspacesChanged": {
                "workspaces": [
                    {
                        "id": 1,
                        "idx": 1,
                        "name": "1",
                        "output": "eDP-1",
                        "is_focused": false,
                        "is_active": false,
                    },
                    {
                        "id": 2,
                        "idx": 2,
                        "name": "2",
                        "output": "HDMI-A-1",
                        "is_focused": true,
                        "is_active": true,
                    },
                ]
            }
        })
    }

    #[test]
    fn active_window_changed_on_inactive_workspace_is_ignored() {
        let shared = Arc::new(SharedState::default());
        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, workspaces_payload());
        assert!(workspace_changed);
        assert!(!window_changed);

        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, windows_payload());
        assert!(workspace_changed);
        assert!(window_changed);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));

        let stale_event = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 201
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, stale_event);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
    }

    #[test]
    fn workspace_active_window_changed_tracks_sorted_focus_position() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let focus_right = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 1,
                "active_window_id": 103
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, focus_right);
        assert!(workspace_changed);
        assert!(window_changed);
        assert_eq!(active_fraction(&shared, 1), Some(1.0));

        let focus_left = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 1,
                "active_window_id": 101
            }
        });
        let (_, window_changed, _) = event_snapshot_changed(&shared, focus_left);
        assert!(window_changed);
        assert_eq!(active_fraction(&shared, 1), Some(1.0 / 3.0));
    }

    #[test]
    fn workspace_active_window_progress_ignores_layout_y_coordinate() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());

        let same_column = json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "first-x",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 9]
                        }
                    },
                    {
                        "id": 202,
                        "title": "second-x",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    }
                ]
            }
        });

        let _ = event_snapshot_changed(&shared, same_column);
        assert_eq!(active_fraction(&shared, 1), Some(1.0));
    }

    #[test]
    fn window_layout_changes_only_affecting_row_position_do_not_change_progress() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());

        let initial_layouts = json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "first-in-column",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 9]
                        }
                    },
                    {
                        "id": 102,
                        "title": "second-in-column",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    }
                ]
            }
        });
        let _ = event_snapshot_changed(&shared, initial_layouts);
        let before = active_fraction(&shared, 1).unwrap_or(0.0);

        let y_noise_only = json!({
            "WindowLayoutsChanged": {
                "changes": [
                    [101, { "pos_in_scrolling_layout": [1, 1] }],
                    [102, { "pos_in_scrolling_layout": [1, 9] }]
                ]
            }
        });
        let _ = event_snapshot_changed(&shared, y_noise_only);
        assert_eq!(active_fraction(&shared, 1), Some(before));
    }

    #[test]
    fn rapid_switch_back_uses_current_workspace_progress() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let activate_workspace_2 = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_2);
        assert!(active_fraction(&shared, 2).is_some());

        let ws2_active = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 201
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_active);
        assert_eq!(active_fraction(&shared, 2), Some(0.0));

        let activate_workspace_1 = json!({
            "WorkspaceActivated": {
                "id": 1,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_1);

        let stale_ws2_focus = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 201
            }
        });
        let _ = event_snapshot_changed(&shared, stale_ws2_focus);

        let ws1_active = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 1,
                "active_window_id": 102
            }
        });
        let _ = event_snapshot_changed(&shared, ws1_active);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
    }

    #[test]
    fn stale_window_focus_change_is_ignored_for_inactive_workspace() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload_multi_ws2());

        let activate_workspace_2 = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_2);

        let ws2_focus_right = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 202
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_focus_right);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));

        let stale_focus_from_ws1 = json!({
            "WindowFocusChanged": {
                "id": 101
            }
        });
        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, stale_focus_from_ws1);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));

        let activate_workspace_1 = json!({
            "WorkspaceActivated": {
                "id": 1,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_1);

        let ws1_focus_center = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 1,
                "active_window_id": 102
            }
        });
        let _ = event_snapshot_changed(&shared, ws1_focus_center);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));

        let stale_focus_from_ws2 = json!({
            "WindowFocusChanged": {
                "id": 202
            }
        });
        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, stale_focus_from_ws2);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
    }

    #[test]
    fn windows_changed_from_inactive_workspace_does_not_reset_active_progress() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload_multi_ws2());

        let activate_workspace_2 = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_2);

        let ws2_focus_right = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 202
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_focus_right);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));

        let stale_windows = json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "ws1-left",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 102,
                        "title": "ws1-mid",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "id": 103,
                        "title": "ws1-right",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [3, 1]
                        }
                    },
                    {
                        "id": 201,
                        "title": "ws2-left",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 202,
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                ]
            }
        });
        let _ = event_snapshot_changed(&shared, stale_windows);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));
    }

    #[test]
    fn switch_to_other_workspace_and_back_restores_previous_active_fraction_without_focus_event() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let ws1_focus_mid = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 1,
                "active_window_id": 102
            }
        });
        let _ = event_snapshot_changed(&shared, ws1_focus_mid);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));

        let activate_workspace_2 = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_2);

        let ws2_focus = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 201
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_focus);
        assert_eq!(active_fraction(&shared, 2), Some(0.0));

        let activate_workspace_1 = json!({
            "WorkspaceActivated": {
                "id": 1,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_workspace_1);

        let stale_windows_without_active_ws1 = json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": 101,
                        "title": "ws1-left",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                    {
                        "id": 102,
                        "title": "ws1-mid",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "id": 103,
                        "title": "ws1-right",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": false,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [3, 1]
                        }
                    },
                    {
                        "id": 201,
                        "title": "ws2-only",
                        "app_id": "a",
                        "workspace_id": 2,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [1, 1]
                        }
                    },
                ]
            }
        });
        let _ = event_snapshot_changed(&shared, stale_windows_without_active_ws1);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
    }

    #[test]
    fn partial_windows_changed_after_workspace_switch_keeps_prior_workspace_progress() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload_ws2_and_ws3());
        let _ = event_snapshot_changed(&shared, windows_payload_ws2_three_ws3_one());

        let ws2_focus_last = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 203
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_focus_last);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));

        let activate_ws3 = json!({
            "WorkspaceActivated": {
                "id": 3,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_ws3);

        let ws3_focus = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 3,
                "active_window_id": 301
            }
        });
        let _ = event_snapshot_changed(&shared, ws3_focus);
        assert_eq!(active_fraction(&shared, 3), Some(0.0));

        let _ = event_snapshot_changed(&shared, windows_payload_ws3_only());

        let activate_ws2 = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, activate_ws2);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));
    }

    #[test]
    fn per_output_workspace_focus_uses_output_active_set_when_not_global_active() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload_dual_outputs());
        let _ = event_snapshot_changed(&shared, windows_payload_dual_outputs());

        let ws2_activate = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": false
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_activate);

        let ws2_focus = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 202
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, ws2_focus);
        assert!(workspace_changed);
        assert!(window_changed);

        let snapshot = shared.workspace_snapshot.read();
        let per_out = snapshot.per_output.get("HDMI-A-1").expect("output entry");
        assert!(per_out.active_workspace.contains(&2));
        drop(snapshot);

        assert_eq!(window_progress_fraction(&shared, 2), Some(1.0));
        let per_output = shared.per_output_window.read();
        let focused = per_output
            .get("HDMI-A-1")
            .expect("HDMI output should have a window entry");
        assert_eq!(focused.title, "ws2-right");
        assert_eq!(focused.output.as_deref(), Some("HDMI-A-1"));
    }

    #[test]
    fn stale_window_focus_events_ignored_when_workspace_inactive_globally_and_per_output() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload_dual_outputs());
        let _ = event_snapshot_changed(&shared, windows_payload_dual_outputs());

        let ws2_activate = json!({
            "WorkspaceActivated": {
                "id": 2,
                "focused": true
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_activate);

        let ws2_focus = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 2,
                "active_window_id": 202
            }
        });
        let _ = event_snapshot_changed(&shared, ws2_focus);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));
        let per_output = shared.per_output_window.read();
        let active_hdmi = per_output
            .get("HDMI-A-1")
            .expect("HDMI output should have a window entry");
        assert_eq!(active_hdmi.title, "ws2-right");
        assert_eq!(active_hdmi.output.as_deref(), Some("HDMI-A-1"));
        let _ = event_snapshot_changed(&shared, workspaces_payload_dual_outputs_ws2_focused_only());

        let stale_focus = json!({
            "WindowFocusChanged": {
                "id": 101
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, stale_focus);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(active_fraction(&shared, 2), Some(1.0));
        assert_eq!(
            shared
                .per_output_window
                .read()
                .get("HDMI-A-1")
                .as_ref()
                .map(|w| w.workspace_id),
            Some(Some(2))
        );
        assert_eq!(
            shared
                .per_output_window
                .read()
                .get("HDMI-A-1")
                .as_ref()
                .map(|w| w.title.as_str()),
            Some("ws2-right")
        );
        assert_eq!(window_progress_fraction(&shared, 2), Some(1.0));
    }

    #[test]
    fn window_closed_updates_window_cache_and_progress() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));

        let close = json!({
            "WindowClosed": {
                "id": 102
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, close);
        assert!(workspace_changed);
        assert!(window_changed);
        assert!(shared.windows.read().get(&102).is_none());
        assert!(shared.focused_window.read().is_none());
        assert_eq!(window_progress_fraction(&shared, 1), Some(0.5));
        assert_eq!(
            shared
                .workspace_snapshot
                .read()
                .window_counts
                .get(&1)
                .copied(),
            Some(2)
        );
    }

    #[test]
    fn malformed_windows_changed_payload_is_tolerated() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());
        let initial_len = shared.windows.read().len();
        assert_eq!(initial_len, 4);

        let malformed = json!({
            "WindowsChanged": {
                "windows": [
                    {
                        "id": "bad-id",
                        "title": "broken",
                        "workspace_id": 1
                    },
                    {
                        "id": 102,
                        "title": "ws1-mid",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [2, 1]
                        }
                    },
                    {
                        "title": "missing-id",
                        "app_id": "a",
                        "workspace_id": 1,
                        "is_focused": true,
                        "is_urgent": false,
                        "layout": {
                            "pos_in_scrolling_layout": [10, 1]
                        }
                    }
                ]
            }
        });

        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, malformed);
        assert!(workspace_changed);
        assert!(window_changed);
        assert_eq!(shared.windows.read().len(), 4);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .and_then(|w| w.workspace_id),
            Some(1)
        );
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .map(|w| w.title.as_str()),
            Some("ws1-mid")
        );

        let malformed_type = json!({
            "WindowsChanged": {
                "windows": "not-an-array"
            }
        });
        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, malformed_type);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.windows.read().len(), 4);
    }

    #[test]
    fn window_closed_triggers_window_update_callbacks_in_expected_order() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let close = json!({
            "WindowClosed": {
                "id": 102
            }
        });

        let (workspace_updates, window_updates, window_list_updates, callback_order) =
            run_event_with_simulated_callbacks(&shared, close);

        assert_eq!(callback_order, vec!["workspace", "window", "window_list"]);
        assert_eq!(workspace_updates.len(), 1);
        assert_eq!(window_updates.len(), 1);
        assert_eq!(window_list_updates.len(), 1);
        assert_eq!(workspace_updates[0].active_workspace.len(), 1);
        assert_eq!(window_updates[0].output.as_deref(), Some("eDP-1"));
        assert_eq!(window_list_updates[0].windows.len(), 3);
        assert_eq!(shared.windows.read().len(), 3);
        assert_eq!(window_progress_fraction(&shared, 1), Some(0.5));
    }

    #[test]
    fn malformed_windows_payload_rejects_callbacks() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let malformed = json!({
            "WindowsChanged": {
                "windows": "not-an-array"
            }
        });

        let (workspace_updates, window_updates, window_list_updates, callback_order) =
            run_event_with_simulated_callbacks(&shared, malformed);

        assert!(callback_order.is_empty());
        assert!(workspace_updates.is_empty());
        assert!(window_updates.is_empty());
        assert!(window_list_updates.is_empty());
        assert_eq!(shared.windows.read().len(), 4);
    }

    #[test]
    fn window_closed_unknown_id_does_not_emit_change_events_or_mutate_state() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let before_snapshot = shared.workspace_snapshot.read().clone();
        let before_windows = shared.windows.read().len();

        let close = json!({
            "WindowClosed": {
                "id": 9_999
            }
        });
        let (workspace_updates, window_updates, window_list_updates, callback_order) =
            run_event_with_simulated_callbacks(&shared, close);

        assert!(
            callback_order.is_empty(),
            "unexpected callback emission for unknown closed window id"
        );
        assert!(workspace_updates.is_empty());
        assert!(window_updates.is_empty());
        assert!(window_list_updates.is_empty());
        assert_eq!(shared.windows.read().len(), before_windows);
        assert_eq!(*shared.workspace_snapshot.read(), before_snapshot);
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .and_then(|w| w.workspace_id),
            Some(1)
        );
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .map(|w| w.title.as_str()),
            Some("ws1-mid")
        );
    }

    #[test]
    fn window_layouts_changed_with_empty_change_set_is_treated_as_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let before_snapshot = shared.workspace_snapshot.read().clone();
        let mut before_windows: Vec<u64> = shared.windows.read().keys().copied().collect();
        before_windows.sort_unstable();

        let layouts = json!({
            "WindowLayoutsChanged": {
                "changes": []
            }
        });

        let (workspace_updates, window_updates, window_list_updates, callback_order) =
            run_event_with_simulated_callbacks(&shared, layouts);

        assert!(
            callback_order.is_empty(),
            "empty layout-changes payload should not emit workspace/window callbacks"
        );
        assert!(workspace_updates.is_empty());
        assert!(window_updates.is_empty());
        assert!(window_list_updates.is_empty());
        assert_eq!(shared.workspace_snapshot.read().clone(), before_snapshot);
        let mut after_windows: Vec<u64> = shared.windows.read().keys().copied().collect();
        after_windows.sort_unstable();
        assert_eq!(after_windows, before_windows);
        assert_eq!(
            shared.windows.read().len(),
            before_windows.len(),
            "window cache size changed for empty layout updates"
        );
    }

    #[test]
    fn spammed_workspace_switches_ignore_stale_focus_payloads() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload_multi_ws2());

        let steps = [
            (2, Some(202), 101, None),
            (1, Some(102), 202, Some(103)),
            (2, Some(201), 102, Some(202)),
            (1, Some(101), 201, Some(101)),
            (2, Some(202), 102, Some(201)),
            (1, Some(103), 201, Some(103)),
        ];

        for (i, step) in steps.iter().enumerate() {
            let (active_ws, active_focus, stale_focus, stale_windows_focus_ws2) = *step;
            let activate = json!({
                "WorkspaceActivated": {
                    "id": active_ws,
                    "focused": true
                }
            });
            let _ = event_snapshot_changed(&shared, activate);

            let active_focus_evt = if let Some(active_focus_id) = active_focus {
                json!({
                    "WorkspaceActiveWindowChanged": {
                        "workspace_id": active_ws,
                        "active_window_id": active_focus_id
                    }
                })
            } else {
                json!({
                    "WorkspaceActiveWindowChanged": {
                        "workspace_id": active_ws,
                        "active_window_id": serde_json::Value::Null
                    }
                })
            };
            let _ = event_snapshot_changed(&shared, active_focus_evt);

            let expected = match active_ws {
                1 => match active_focus {
                    Some(101) => Some(1.0 / 3.0),
                    Some(102) => Some(2.0 / 3.0),
                    Some(103) => Some(1.0),
                    _ => Some(0.0),
                },
                2 => match active_focus {
                    Some(201) => Some(0.5),
                    Some(202) => Some(1.0),
                    _ => Some(0.0),
                },
                _ => None,
            };

            assert_eq!(active_fraction(&shared, active_ws), expected);

            let stale_focus_evt = json!({
                "WindowFocusChanged": {
                    "id": stale_focus
                }
            });
            let _ = event_snapshot_changed(&shared, stale_focus_evt);
            assert_eq!(active_fraction(&shared, active_ws), expected);

            let stale_windows_focus_id = stale_windows_focus_ws2.or(Some(stale_focus));
            let stale_windows_payload = json!({
                "WindowsChanged": {
                    "windows": [
                        {
                            "id": 101,
                            "title": "ws1-left",
                            "app_id": "a",
                            "workspace_id": 1,
                            "is_focused": stale_focus == 101 && stale_windows_focus_id == Some(101),
                            "is_urgent": false,
                            "layout": {
                                "pos_in_scrolling_layout": [1, 1]
                            }
                        },
                        {
                            "id": 102,
                            "title": "ws1-mid",
                            "app_id": "a",
                            "workspace_id": 1,
                            "is_focused": stale_focus == 102 && stale_windows_focus_id == Some(102),
                            "is_urgent": false,
                            "layout": {
                                "pos_in_scrolling_layout": [2, 1]
                            }
                        },
                        {
                            "id": 103,
                            "title": "ws1-right",
                            "app_id": "a",
                            "workspace_id": 1,
                            "is_focused": stale_focus == 103 && stale_windows_focus_id == Some(103),
                            "is_urgent": false,
                            "layout": {
                                "pos_in_scrolling_layout": [3, 1]
                            }
                        },
                        {
                            "id": 201,
                            "title": "ws2-left",
                            "app_id": "a",
                            "workspace_id": 2,
                            "is_focused": stale_focus == 201 && stale_windows_focus_id == Some(201),
                            "is_urgent": false,
                            "layout": {
                                "pos_in_scrolling_layout": [1, 1]
                            }
                        },
                        {
                            "id": 202,
                            "title": "ws2-right",
                            "app_id": "a",
                            "workspace_id": 2,
                            "is_focused": stale_focus == 202 && stale_windows_focus_id == Some(202),
                            "is_urgent": false,
                            "layout": {
                                "pos_in_scrolling_layout": [2, 1]
                            }
                        },
                    ]
                }
            });
            let _ = event_snapshot_changed(&shared, stale_windows_payload);
            assert_eq!(active_fraction(&shared, active_ws), expected);

            if i == steps.len() - 1 {
                continue;
            }
        }
    }

    #[test]
    fn workspace_activated_redundant_event_without_state_change_is_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());

        let focus_true = json!({
            "WorkspaceActivated": {
                "id": 1,
                "focused": true
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, focus_true);
        assert!(!workspace_changed);
        assert!(!window_changed);

        let focus_false = json!({
            "WorkspaceActivated": {
                "id": 1,
                "focused": false
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, focus_false);
        assert!(!workspace_changed);
        assert!(!window_changed);
    }

    #[test]
    fn workspace_active_window_changed_repeated_focus_is_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let set_focus_mid = json!({
            "WorkspaceActiveWindowChanged": {
                "workspace_id": 1,
                "active_window_id": 102
            }
        });
        let _ = event_snapshot_changed(&shared, set_focus_mid.clone());

        let before_snapshot = shared.workspace_snapshot.read().clone();
        let before_focused = shared
            .focused_window
            .read()
            .as_ref()
            .and_then(|w| Some((w.workspace_id, w.title.clone())));

        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, set_focus_mid.clone());
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.workspace_snapshot.read().clone(), before_snapshot);
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .and_then(|w| Some((w.workspace_id, w.title.clone()))),
            before_focused
        );
    }

    #[test]
    fn window_opened_or_changed_missing_id_is_a_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let before_snapshot = shared.workspace_snapshot.read().clone();
        let before_windows = shared.windows.read().len();

        let malformed = json!({
            "WindowOpenedOrChanged": {
                "window": {
                    "title": "mystery",
                    "app_id": "x",
                    "workspace_id": 1,
                    "is_focused": true,
                    "is_urgent": false
                }
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, malformed);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.windows.read().len(), before_windows);
        assert_eq!(*shared.workspace_snapshot.read(), before_snapshot);
    }

    #[test]
    fn window_opened_or_changed_inactive_workspace_focus_does_not_change_global_focus() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .and_then(|w| Some(w.title.as_str())),
            Some("ws1-mid")
        );

        let new_window = json!({
            "WindowOpenedOrChanged": {
                "window": {
                    "id": 999,
                    "title": "ws2-new",
                    "app_id": "b",
                    "workspace_id": 2,
                    "is_focused": true,
                    "is_urgent": false,
                    "layout": {
                        "pos_in_scrolling_layout": [1, 1]
                    }
                }
            }
        });
        let _ = event_snapshot_changed(&shared, new_window);
        assert_eq!(active_fraction(&shared, 1), Some(2.0 / 3.0));
        assert_eq!(
            shared
                .focused_window
                .read()
                .as_ref()
                .and_then(|w| Some(w.title.as_str())),
            Some("ws1-mid")
        );
    }

    #[test]
    fn workspace_urgency_changed_unknown_workspace_is_ignored() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());
        let before = shared.workspace_snapshot.read().urgent_workspaces.clone();

        let unknown_urgent = json!({
            "WorkspaceUrgencyChanged": {
                "id": 99,
                "urgent": true
            }
        });
        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, unknown_urgent);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.workspace_snapshot.read().urgent_workspaces, before);
    }

    #[test]
    fn keyboard_layout_switch_noop_when_unchanged() {
        let shared = Arc::new(SharedState::default());
        let init = json!({
            "KeyboardLayoutsChanged": {
                "keyboard_layouts": {
                    "names": ["English", "Deutsch"],
                    "current_idx": 0
                }
            }
        });
        let (_, _, kb_changed) = event_snapshot_changed(&shared, init);
        assert!(kb_changed);

        let same = json!({
            "KeyboardLayoutSwitched": {
                "idx": 0
            }
        });
        let (_, _, kb_changed) = event_snapshot_changed(&shared, same);
        assert!(!kb_changed);
    }

    #[test]
    fn keyboard_layouts_changed_noop_when_payload_is_identical() {
        let shared = Arc::new(SharedState::default());
        let init = json!({
            "KeyboardLayoutsChanged": {
                "keyboard_layouts": {
                    "names": ["English", "Deutsch"],
                    "current_idx": 0
                }
            }
        });
        let (_, _, kb_changed) = event_snapshot_changed(&shared, init.clone());
        assert!(kb_changed);

        let (_, _, kb_changed) = event_snapshot_changed(&shared, init);
        assert!(!kb_changed);
    }

    #[test]
    fn windows_changed_repeated_payload_is_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());
        let before = shared.workspace_snapshot.read().clone();

        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, windows_payload());
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.workspace_snapshot.read().clone(), before);
    }

    #[test]
    fn workspaces_changed_repeated_payload_is_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let before = shared.workspace_snapshot.read().clone();

        let (workspace_changed, window_changed, _) =
            event_snapshot_changed(&shared, workspaces_payload());
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.workspace_snapshot.read().clone(), before);
    }

    #[test]
    fn window_opened_or_changed_repeated_payload_is_noop() {
        let shared = Arc::new(SharedState::default());
        let _ = event_snapshot_changed(&shared, workspaces_payload());
        let _ = event_snapshot_changed(&shared, windows_payload());

        let before = shared.workspace_snapshot.read().clone();
        let before_window = shared
            .windows
            .read()
            .get(&102)
            .cloned()
            .expect("expected window 102");

        let open = json!({
            "WindowOpenedOrChanged": {
                "window": {
                    "id": 102,
                    "title": "ws1-mid",
                    "app_id": "a",
                    "workspace_id": 1,
                    "is_focused": true,
                    "is_urgent": false,
                    "layout": {
                        "pos_in_scrolling_layout": [2, 1]
                    }
                }
            }
        });
        let (workspace_changed, window_changed, _) = event_snapshot_changed(&shared, open);
        assert!(!workspace_changed);
        assert!(!window_changed);
        assert_eq!(shared.workspace_snapshot.read().clone(), before);
        let after_window = shared
            .windows
            .read()
            .get(&102)
            .expect("expected window 102")
            .clone();

        assert_eq!(after_window.id, before_window.id);
        assert_eq!(after_window.title, before_window.title);
        assert_eq!(after_window.app_id, before_window.app_id);
        assert_eq!(after_window.workspace_id, before_window.workspace_id);
        assert_eq!(after_window.is_focused, before_window.is_focused);
        assert_eq!(after_window.is_urgent, before_window.is_urgent);
        assert_eq!(after_window.layout_position, before_window.layout_position);
    }
}
