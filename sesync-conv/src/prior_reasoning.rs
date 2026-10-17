//! Thinking carried across harnesses as marked text.
//!
//! A reasoning block cannot move to another harness or model: Claude signs its thinking
//! and rejects foreign blocks, and other models cannot resume a reasoning trajectory they
//! did not produce. So when a reader sees thinking text from a different model, it keeps
//! the words but wraps them in a plain-text marker, placed before the answer:
//!
//! ```text
//! <prior_reasoning harness="omp" model="claude-opus">
//! ...the original thinking text...
//! </prior_reasoning>
//! The actual answer text...
//! ```
//!
//! The tag is deliberately not `<thinking>`: the next model reads it as an earlier model's
//! notes, not as its own reasoning. Signatures are never copied. Redacted thinking has no
//! text and stays dropped. A converter must never wrap a block that is already wrapped, so
//! a round trip (A -> B -> A) keeps one marker, not nested ones.
//! Design from session-migrate (xhluca, MIT) and docs/research/session-fidelity.md.

/// Wrap thinking text in the marker. Empty thinking returns None (nothing to keep).
pub fn wrap(text: &str, harness: &str, model: &str) -> Option<String> {
    let body = text.trim();
    if body.is_empty() {
        return None;
    }
    if is_wrapped(body) {
        // Already a marker from an earlier conversion: keep it exactly as it is.
        return Some(body.to_string());
    }
    Some(format!(
        "<prior_reasoning harness=\"{}\" model=\"{}\">\n{}\n</prior_reasoning>",
        escape_attr(harness),
        escape_attr(model),
        body
    ))
}

/// True when the whole text is already one `<prior_reasoning>` block.
pub fn is_wrapped(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("<prior_reasoning ") && t.ends_with("</prior_reasoning>")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_text_with_source_harness_and_model() {
        let w = wrap("I should list the files first.", "omp", "claude-opus").unwrap();
        assert!(w.starts_with("<prior_reasoning harness=\"omp\" model=\"claude-opus\">\n"));
        assert!(w.contains("I should list the files first."));
        assert!(w.ends_with("</prior_reasoning>"));
    }

    #[test]
    fn empty_thinking_is_nothing_to_keep() {
        assert_eq!(wrap("", "omp", "m"), None);
        assert_eq!(wrap("   \n ", "omp", "m"), None);
    }

    #[test]
    fn an_existing_marker_is_not_wrapped_again() {
        let once = wrap("notes", "omp", "m").unwrap();
        let twice = wrap(&once, "claude", "other").unwrap();
        assert_eq!(twice, once, "a round trip must keep one marker, not nest them");
        assert_eq!(twice.matches("<prior_reasoning").count(), 1);
    }

    #[test]
    fn attribute_values_cannot_break_out_of_the_tag() {
        let w = wrap("x", "omp\" injected=\"yes", "m<script>").unwrap();
        assert!(!w.contains("injected=\"yes\""), "quote in an attribute must be escaped: {w}");
        assert!(w.contains("&lt;script&gt;"));
    }

    #[test]
    fn is_wrapped_recognises_only_a_full_marker() {
        assert!(is_wrapped("<prior_reasoning harness=\"a\" model=\"b\">\nx\n</prior_reasoning>"));
        assert!(!is_wrapped("plain text"));
        assert!(!is_wrapped("<prior_reasoning harness=\"a\" model=\"b\">\nx"), "unterminated is not a marker");
    }
}
