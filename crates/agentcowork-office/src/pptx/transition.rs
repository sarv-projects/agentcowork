//! PPT transitions/animations (D7-gap — doc 63 §3). Read and write
//! `p:transition` on a slide; existing transition XML is preserved verbatim
//! unless explicitly replaced (never corrupt an authored deck).

use roxmltree::Document;
use thiserror::Error;

/// PresentationML namespace.
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    Fade,
    Wipe,
    Push,
    Cut,
    None,
}

impl TransitionKind {
    /// The element local name in `p:transition`.
    pub fn as_str(self) -> &'static str {
        match self {
            TransitionKind::Fade => "fade",
            TransitionKind::Wipe => "wipe",
            TransitionKind::Push => "push",
            TransitionKind::Cut => "cut",
            TransitionKind::None => "",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "fade" => TransitionKind::Fade,
            "wipe" => TransitionKind::Wipe,
            "push" => TransitionKind::Push,
            "cut" => TransitionKind::Cut,
            _ => TransitionKind::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub kind: TransitionKind,
    /// `spd` attribute: slow / med / fast.
    pub speed: Option<String>,
    /// Advance timing (in ms) if present.
    pub advance_ms: Option<u64>,
}

#[derive(Debug, Error)]
pub enum TransitionError {
    #[error("xml parse error: {0}")]
    Parse(#[from] roxmltree::Error),
    #[error("no <p:sld> root in slide")]
    NoSlideRoot,
}

/// Read the slide's transition (if any).
pub fn extract_transition(slide_xml: &str) -> Result<Option<Transition>, TransitionError> {
    let doc = Document::parse(slide_xml)?;
    let Some(sld) = doc.descendants().find(|d| {
        d.is_element() && d.tag_name().namespace() == Some(P) && d.tag_name().name() == "sld"
    }) else {
        return Err(TransitionError::NoSlideRoot);
    };
    for child in sld.children() {
        if child.is_element()
            && child.tag_name().namespace() == Some(P)
            && child.tag_name().name() == "transition"
        {
            let speed = child.attribute("spd").map(str::to_string);
            let advance_ms = child.attribute("advTm").and_then(|v| v.parse::<u64>().ok());
            // The transition kind is a child element (p:fade, p:wipe, …).
            let kind = child
                .children()
                .find(|c| c.is_element() && c.tag_name().namespace() == Some(P))
                .map(|c| TransitionKind::from_name(c.tag_name().name()))
                .unwrap_or(TransitionKind::None);
            return Ok(Some(Transition {
                kind,
                speed,
                advance_ms,
            }));
        }
    }
    Ok(None)
}

/// Insert (or replace) the slide's `p:transition`. The new transition is
/// placed before the slide's content (`p:cSld`) per the schema order.
pub fn set_transition(slide_xml: &str, t: &Transition) -> Result<String, TransitionError> {
    let doc = Document::parse(slide_xml)?;
    let Some(sld) = doc.descendants().find(|d| {
        d.is_element() && d.tag_name().namespace() == Some(P) && d.tag_name().name() == "sld"
    }) else {
        return Err(TransitionError::NoSlideRoot);
    };

    let kind_el = t.kind.as_str();
    let speed = t
        .speed
        .as_deref()
        .map(|s| format!(" spd=\"{s}\""))
        .unwrap_or_default();
    let adv = t
        .advance_ms
        .map(|ms| format!(" advTm=\"{ms}\""))
        .unwrap_or_default();
    let new_transition = if kind_el.is_empty() {
        format!("<p:transition{speed}{adv}/>")
    } else {
        format!("<p:transition{speed}{adv}><p:{kind_el}/></p:transition>")
    };

    // Replace an existing transition in place (byte-range splice).
    if let Some(existing) = sld.children().find(|c| {
        c.is_element() && c.tag_name().namespace() == Some(P) && c.tag_name().name() == "transition"
    }) {
        let range = existing.range();
        let mut out = String::with_capacity(slide_xml.len());
        out.push_str(&slide_xml[..range.start]);
        out.push_str(&new_transition);
        out.push_str(&slide_xml[range.end..]);
        return Ok(out);
    }

    // No existing transition: insert right after the opening <p:sld ...> tag.
    let open = sld.range();
    let open_tag_end = slide_xml[open.start..]
        .find('>')
        .map(|i| open.start + i + 1)
        .ok_or(TransitionError::NoSlideRoot)?;
    let mut out = String::with_capacity(slide_xml.len() + new_transition.len());
    out.push_str(&slide_xml[..open_tag_end]);
    out.push_str(&new_transition);
    out.push_str(&slide_xml[open_tag_end..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLIDE: &str = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree/></p:cSld></p:sld>"#;

    #[test]
    fn no_transition_returns_none() {
        assert_eq!(extract_transition(SLIDE).unwrap(), None);
    }

    #[test]
    fn extracts_fade_transition() {
        let xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:transition spd="slow"><p:fade/></p:transition><p:cSld><p:spTree/></p:cSld></p:sld>"#;
        let t = extract_transition(xml).unwrap().unwrap();
        assert_eq!(t.kind, TransitionKind::Fade);
        assert_eq!(t.speed.as_deref(), Some("slow"));
    }

    #[test]
    fn set_transition_inserts_before_content() {
        let out = set_transition(
            SLIDE,
            &Transition {
                kind: TransitionKind::Wipe,
                speed: Some("med".into()),
                advance_ms: Some(1500),
            },
        )
        .unwrap();
        assert!(
            out.contains("<p:transition spd=\"med\" advTm=\"1500\"><p:wipe/></p:transition>"),
            "{out}"
        );
        // Content still present, transition comes first.
        assert!(out.find("p:transition").unwrap() < out.find("p:cSld").unwrap());
        assert!(Document::parse(&out).is_ok());
    }

    #[test]
    fn set_transition_replaces_existing() {
        let xml = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:transition spd="slow"><p:fade/></p:transition><p:cSld/></p:sld>"#;
        let out = set_transition(
            xml,
            &Transition {
                kind: TransitionKind::Push,
                speed: None,
                advance_ms: None,
            },
        )
        .unwrap();
        assert!(out.contains("<p:push/>"), "{out}");
        assert!(!out.contains("<p:fade/>"), "{out}");
    }
}
