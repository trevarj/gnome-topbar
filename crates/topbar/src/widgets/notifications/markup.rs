//! Notification bodies, made safe for Pango.
//!
//! The daemon advertises `body-markup`, so senders are entitled to put
//! `<b>bold</b>` in a body — and, in practice, to put unescaped `&`, unclosed
//! tags, and whole `<img>` elements in one too. Pango's parser rejects the lot
//! and GTK then draws nothing, so the body is rewritten here into a small
//! guaranteed-valid subset before it reaches a label.
//!
//! [`sanitize`] is best effort; [`apply`] is the guarantee. If the rewritten
//! markup still will not parse, the label is given the plain text instead of
//! an empty line.

use gtk4::{Label, pango};

/// The inline tags Pango understands that a notification may use.
///
/// `a` is deliberately absent: a link the panel cannot open is a lie, and
/// clicking a toast already means something else.
const ALLOWED: &[&str] = &["b", "i", "u", "s"];

/// Put `body` on `label`, as markup when it can be and as text when it cannot.
pub fn apply(label: &Label, body: &str) {
    let markup = sanitize(body);
    if pango::parse_markup(&markup, '\0').is_ok() {
        label.set_markup(&markup);
    } else {
        // Belt and braces: whatever the sender did, the user still reads the
        // words rather than a blank line.
        label.set_text(body);
    }
}

/// Rewrite `body` as Pango markup: allowed tags kept, everything else escaped.
///
/// Unclosed tags are closed at the end and mismatched closing tags are escaped
/// rather than dropped, so the result parses even when the input is nonsense.
pub fn sanitize(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut open: Vec<&'static str> = Vec::new();
    let mut rest = body;

    while let Some(at) = rest.find(['<', '&', '>']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];

        match rest.as_bytes()[0] {
            b'>' => {
                out.push_str("&gt;");
                rest = &rest[1..];
            }
            b'&' => match entity(rest) {
                Some(len) => {
                    out.push_str(&rest[..len]);
                    rest = &rest[len..];
                }
                None => {
                    out.push_str("&amp;");
                    rest = &rest[1..];
                }
            },
            _ => match tag(rest) {
                Some((Tag::Break, len)) => {
                    out.push('\n');
                    rest = &rest[len..];
                }
                Some((Tag::Open(name), len)) => {
                    out.push('<');
                    out.push_str(name);
                    out.push('>');
                    open.push(name);
                    rest = &rest[len..];
                }
                // Only a close that matches the innermost open tag is honoured;
                // anything else would produce mis-nested markup.
                Some((Tag::Close(name), len)) if open.last() == Some(&name) => {
                    out.push_str("</");
                    out.push_str(name);
                    out.push('>');
                    open.pop();
                    rest = &rest[len..];
                }
                _ => {
                    out.push_str("&lt;");
                    rest = &rest[1..];
                }
            },
        }
    }
    out.push_str(rest);

    for name in open.iter().rev() {
        out.push_str("</");
        out.push_str(name);
        out.push('>');
    }
    out
}

/// One recognised tag.
enum Tag {
    /// `<b>` and friends.
    Open(&'static str),
    /// `</b>` and friends.
    Close(&'static str),
    /// `<br>`, `<br/>`, `<br />`.
    Break,
}

/// Read a tag at the start of `rest`, with the bytes it occupies.
///
/// Nothing here is trimmed, deliberately: `a < b > c` is arithmetic that
/// happens to look like a tag, and treating it as one would turn half a
/// sentence bold. A real tag has no spaces in it.
fn tag(rest: &str) -> Option<(Tag, usize)> {
    let end = rest.find('>')?;
    let inner = &rest[1..end];
    let len = end + 1;

    if ["br", "br/", "br /"]
        .iter()
        .any(|form| inner.eq_ignore_ascii_case(form))
    {
        return Some((Tag::Break, len));
    }

    let (closing, name) = match inner.strip_prefix('/') {
        Some(name) => (true, name),
        None => (false, inner),
    };
    let name = ALLOWED
        .iter()
        .find(|allowed| name.eq_ignore_ascii_case(allowed))?;

    Some((
        if closing {
            Tag::Close(name)
        } else {
            Tag::Open(name)
        },
        len,
    ))
}

/// Length of a well-formed XML entity at the start of `rest`, if there is one.
fn entity(rest: &str) -> Option<usize> {
    let end = rest[..rest.len().min(12)].find(';')?;
    let name = &rest[1..end];
    let known = matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
        || (name.starts_with('#')
            && name.len() > 1
            && name[1..].chars().all(|c| c.is_ascii_alphanumeric()));
    known.then_some(end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every sanitised body must parse, which is the whole point.
    fn parses(markup: &str) -> bool {
        pango::parse_markup(markup, '\0').is_ok()
    }

    #[test]
    fn plain_text_is_left_alone() {
        assert_eq!(sanitize("see you at six"), "see you at six");
    }

    #[test]
    fn allowed_tags_survive() {
        assert_eq!(sanitize("<b>Ada</b> replied"), "<b>Ada</b> replied");
        assert_eq!(
            sanitize("<i>x</i> <u>y</u> <s>z</s>"),
            "<i>x</i> <u>y</u> <s>z</s>"
        );
        assert_eq!(
            sanitize("<B>shouty</B>"),
            "<b>shouty</b>",
            "case is normalised"
        );
    }

    #[test]
    fn everything_else_is_escaped() {
        assert_eq!(
            sanitize(r#"<img src="x"> <script>bad</script>"#),
            "&lt;img src=\"x\"&gt; &lt;script&gt;bad&lt;/script&gt;"
        );
        assert_eq!(sanitize("a < b > c"), "a &lt; b &gt; c");
    }

    #[test]
    fn bare_ampersands_are_escaped_and_entities_are_kept() {
        assert_eq!(sanitize("Tom & Jerry"), "Tom &amp; Jerry");
        assert_eq!(sanitize("a &amp; b &lt; c &#39;"), "a &amp; b &lt; c &#39;");
        assert_eq!(sanitize("&notanentity"), "&amp;notanentity");
    }

    #[test]
    fn line_breaks_become_newlines() {
        assert_eq!(
            sanitize("one<br>two<br/>three<br />four"),
            "one\ntwo\nthree\nfour"
        );
    }

    #[test]
    fn unclosed_tags_are_closed_at_the_end() {
        assert_eq!(sanitize("<b>never ends"), "<b>never ends</b>");
        assert_eq!(sanitize("<b><i>both"), "<b><i>both</i></b>");
    }

    #[test]
    fn a_mismatched_close_is_escaped_rather_than_honoured() {
        assert_eq!(sanitize("<b>x</i>y"), "<b>x&lt;/i&gt;y</b>");
        assert_eq!(sanitize("</b>"), "&lt;/b&gt;");
    }

    #[test]
    fn every_sanitised_body_parses() {
        for body in [
            "plain",
            "<b>bold</b>",
            "<b>unclosed",
            "</i>stray",
            "Tom & Jerry",
            "<img src=x onerror=y>",
            "a<br>b",
            "<<<>>>&&&",
            "<b><i><u><s>deep</s></u></i></b>",
            "<b",
            "&#x41;",
            "",
        ] {
            let markup = sanitize(body);
            assert!(
                parses(&markup),
                "`{body}` produced `{markup}`, which will not parse"
            );
        }
    }

    #[test]
    fn text_is_preserved_even_when_the_markup_is_stripped() {
        // The words a sender wrote must survive; only the tags are negotiable.
        let markup = sanitize("<marquee>hello</marquee>");
        assert!(markup.contains("hello"));
    }
}
