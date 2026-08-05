//! Deciding, from PipeWire's node graph, whether a screen is being shared.
//!
//! Pure: a set of nodes and a set of links in, one boolean out. That is the
//! whole reason this is separate from the thread that talks to PipeWire — the
//! heuristic is the part that is easy to get wrong and impossible to test
//! against a live session without asking somebody to share their screen.
//!
//! ## The heuristic, and where it comes from
//!
//! v1 answered this by running `pw-dump` once a second and reading its JSON.
//! The polling is the defect this replaces; the *analysis* was right and is
//! ported field for field.
//!
//! A screen is being shared when there is an **active link** whose **output**
//! is a video source that is not a camera. Each clause is doing work:
//!
//! - *Active link*, not merely a link: a node the portal created for a session
//!   the user then cancelled still exists, with a link in `paused`. The dot has
//!   to mean "right now".
//! - *Output side*, not input: the consumer's identity is irrelevant. A browser
//!   tab, OBS and a remote-desktop daemon are all the same fact.
//! - *Not a camera*: `Video/Source` is also what every webcam publishes, and a
//!   dot that came on when somebody joined a video call without sharing
//!   anything would teach the user to ignore it.
//!
//! The camera test is by name, which is a heuristic and is admitted as one:
//! `v4l2`, `camera`, `webcam`, `camlink` in the node's name, description or
//! application name. It is the same list v1 used. The failure modes are a
//! camera whose driver names it something else (a dot that should not be there)
//! and a screen cast from an application with "camera" in its name (a dot that
//! should be). Both are rare; both are better than the alternative, which is a
//! dot that is on during every video call.

use std::collections::{HashMap, HashSet};

/// One node in the graph, as much of it as the decision needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    /// `media.class` — `Video/Source`, `Stream/Output/Video`, `Audio/Sink`, …
    pub media_class: Option<String>,
    /// `node.name`.
    pub name: Option<String>,
    /// `node.description`.
    pub description: Option<String>,
    /// `application.name`.
    pub application: Option<String>,
}

impl Node {
    /// Whether this node is something casting a screen.
    ///
    /// The two classes are what a screen cast looks like from either side of
    /// PipeWire's own vocabulary: `Video/Source` is the node
    /// xdg-desktop-portal publishes, and `Stream/Output/Video` is what a
    /// compositor streaming directly registers. `Stream/Input/Video` — the
    /// *consumer* — is deliberately not here.
    pub fn is_screen_cast(&self) -> bool {
        let casting = matches!(
            self.media_class.as_deref(),
            Some("Video/Source" | "Stream/Output/Video")
        );
        casting && !self.looks_like_a_camera()
    }

    /// Whether this node is a camera rather than a screen.
    fn looks_like_a_camera(&self) -> bool {
        let haystack = [
            self.name.as_deref(),
            self.description.as_deref(),
            self.application.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<String>>()
        .join(" ");

        ["v4l2", "camera", "webcam", "camlink"]
            .iter()
            .any(|marker| haystack.contains(marker))
    }
}

/// One link between two nodes, and whether it is carrying anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// The node the data comes from.
    pub output: u32,
    /// The node it goes to. Kept for the log line, not for the decision.
    pub input: u32,
    /// Whether PipeWire says the link is running.
    pub active: bool,
}

/// The graph as the listener has seen it so far.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: HashMap<u32, Node>,
    links: HashMap<u32, Link>,
}

impl Graph {
    /// An empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember one node.
    pub fn add_node(&mut self, id: u32, node: Node) {
        self.nodes.insert(id, node);
    }

    /// Remember one link, or update the one already there.
    pub fn add_link(&mut self, id: u32, link: Link) {
        self.links.insert(id, link);
    }

    /// Move one link's state without disturbing what it joins.
    ///
    /// A link's `state` arrives *after* the link itself does, in a separate
    /// event, which is why this is not part of [`Graph::add_link`].
    pub fn set_link_active(&mut self, id: u32, active: bool) {
        if let Some(link) = self.links.get_mut(&id) {
            link.active = active;
        }
    }

    /// Forget an object that has gone.
    ///
    /// One method for both, because the registry announces removals by id and
    /// does not say what kind of thing the id was.
    pub fn remove(&mut self, id: u32) {
        self.nodes.remove(&id);
        self.links.remove(&id);
    }

    /// Whether a screen is being shared right now.
    pub fn screen_sharing(&self) -> bool {
        let casting: HashSet<u32> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.is_screen_cast())
            .map(|(id, _)| *id)
            .collect();
        self.links
            .values()
            .any(|link| link.active && casting.contains(&link.output))
    }

    /// What is being shared, for a log line. Debug builds' aid, not the dot's.
    pub fn describe(&self) -> Option<String> {
        let link = self.links.values().find(|link| {
            link.active
                && self
                    .nodes
                    .get(&link.output)
                    .is_some_and(Node::is_screen_cast)
        })?;
        let node = self.nodes.get(&link.output)?;
        Some(
            node.description
                .clone()
                .or_else(|| node.name.clone())
                .unwrap_or_else(|| format!("node {}", link.output)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: &str, name: &str) -> Node {
        Node {
            media_class: Some(class.to_string()),
            name: Some(name.to_string()),
            ..Node::default()
        }
    }

    fn active(output: u32, input: u32) -> Link {
        Link {
            output,
            input,
            active: true,
        }
    }

    /// v1's first fixture: the portal casting into a browser.
    #[test]
    fn an_active_link_out_of_a_portal_node_is_a_screen_being_shared() {
        let mut graph = Graph::new();
        graph.add_node(10, node("Video/Source", "portal-screen-cast"));
        graph.add_node(20, node("Stream/Input/Video", "Browser"));
        graph.add_link(30, active(10, 20));

        assert!(graph.screen_sharing());
        assert_eq!(graph.describe().as_deref(), Some("portal-screen-cast"));
    }

    /// v1's second fixture: a webcam, which is not a screen.
    #[test]
    fn a_camera_on_a_video_call_does_not_raise_the_dot() {
        let mut graph = Graph::new();
        graph.add_node(
            10,
            Node {
                media_class: Some("Video/Source".into()),
                name: Some("v4l2_input.usb-camera".into()),
                description: Some("Integrated Camera".into()),
                ..Node::default()
            },
        );
        graph.add_node(20, node("Stream/Input/Video", "Browser"));
        graph.add_link(30, active(10, 20));

        assert!(
            !graph.screen_sharing(),
            "a dot that came on for every video call is a dot nobody reads"
        );
    }

    /// v1's third fixture: a compositor streaming into OBS.
    #[test]
    fn a_compositor_streaming_directly_counts_too() {
        let mut graph = Graph::new();
        graph.add_node(10, node("Stream/Output/Video", "niri"));
        graph.add_node(20, node("Stream/Input/Video", "obs"));
        graph.add_link(30, active(10, 20));
        assert!(graph.screen_sharing());
    }

    #[test]
    fn every_word_the_camera_test_looks_for_is_looked_for() {
        for name in [
            "v4l2_input.pci-0000_00_14",
            "Integrated Camera",
            "USB Webcam",
            "Elgato Cam Link 4K camlink",
        ] {
            let node = node("Video/Source", name);
            assert!(!node.is_screen_cast(), "{name} should read as a camera");
        }
        // And the field it is in does not matter.
        let by_description = Node {
            media_class: Some("Video/Source".into()),
            name: Some("alsa_input.7".into()),
            description: Some("HD WebCam".into()),
            ..Node::default()
        };
        assert!(!by_description.is_screen_cast());
        let by_application = Node {
            media_class: Some("Video/Source".into()),
            application: Some("Cheese Camera".into()),
            ..Node::default()
        };
        assert!(!by_application.is_screen_cast());
    }

    #[test]
    fn a_node_with_no_link_is_a_session_nobody_is_watching() {
        // What a portal node looks like between the picker opening and the
        // consumer attaching, and what it looks like after the consumer left.
        let mut graph = Graph::new();
        graph.add_node(10, node("Video/Source", "portal-screen-cast"));
        assert!(!graph.screen_sharing());
        assert!(graph.describe().is_none());
    }

    #[test]
    fn a_link_that_is_not_running_does_not_count() {
        let mut graph = Graph::new();
        graph.add_node(10, node("Video/Source", "portal-screen-cast"));
        graph.add_node(20, node("Stream/Input/Video", "Browser"));
        graph.add_link(
            30,
            Link {
                output: 10,
                input: 20,
                active: false,
            },
        );
        assert!(
            !graph.screen_sharing(),
            "a paused link is a session that was cancelled"
        );

        // The state arrives after the link does, in an event of its own.
        graph.set_link_active(30, true);
        assert!(graph.screen_sharing());
        graph.set_link_active(30, false);
        assert!(!graph.screen_sharing());
    }

    #[test]
    fn the_consumers_identity_is_irrelevant() {
        // A browser tab, OBS and a remote-desktop daemon are the same fact, so
        // the input side is never looked at.
        let mut graph = Graph::new();
        graph.add_node(10, node("Video/Source", "portal-screen-cast"));
        graph.add_link(30, active(10, 999));
        assert!(graph.screen_sharing(), "even a consumer nobody has seen");
    }

    #[test]
    fn audio_is_not_video() {
        let mut graph = Graph::new();
        graph.add_node(10, node("Audio/Source", "alsa_input.pci-0000"));
        graph.add_node(11, node("Stream/Output/Audio", "Firefox"));
        graph.add_link(30, active(10, 20));
        graph.add_link(31, active(11, 21));
        assert!(!graph.screen_sharing());
    }

    #[test]
    fn a_consumer_is_not_a_source_however_the_link_runs() {
        // `Stream/Input/Video` on the output side of a link is a node feeding
        // something else its *input*, which is not a screen being cast.
        let mut graph = Graph::new();
        graph.add_node(10, node("Stream/Input/Video", "obs"));
        graph.add_link(30, active(10, 20));
        assert!(!graph.screen_sharing());
    }

    #[test]
    fn a_session_ending_takes_the_dot_with_it() {
        let mut graph = Graph::new();
        graph.add_node(10, node("Video/Source", "portal-screen-cast"));
        graph.add_node(20, node("Stream/Input/Video", "Browser"));
        graph.add_link(30, active(10, 20));
        assert!(graph.screen_sharing());

        // The link goes first, then the node — which is the order PipeWire
        // tears a portal session down in.
        graph.remove(30);
        assert!(!graph.screen_sharing());
        graph.remove(10);
        assert!(!graph.screen_sharing());
        graph.remove(20);
        assert!(!graph.screen_sharing());
    }

    #[test]
    fn an_empty_graph_says_nothing_is_happening() {
        assert!(!Graph::new().screen_sharing());
    }

    #[test]
    fn a_node_with_no_class_at_all_is_not_a_screen() {
        let mut graph = Graph::new();
        graph.add_node(10, Node::default());
        graph.add_link(30, active(10, 20));
        assert!(!graph.screen_sharing());
    }

    #[test]
    fn two_sessions_at_once_are_still_one_dot() {
        let mut graph = Graph::new();
        graph.add_node(10, node("Video/Source", "portal-screen-cast-1"));
        graph.add_node(11, node("Video/Source", "portal-screen-cast-2"));
        graph.add_link(30, active(10, 20));
        graph.add_link(31, active(11, 21));
        assert!(graph.screen_sharing());
        // And ending one leaves the other.
        graph.remove(30);
        assert!(graph.screen_sharing());
    }
}
