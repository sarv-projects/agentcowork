//! PPT animations (`p:anim` — the D7-gap remainder of doc 63 §3). Builds a
//! `p:timing` block that targets one shape (`p:spTgt spid=…`) with a
//! fade/zoom/appear effect, in the schema order PowerPoint expects. The
//! existing transition module handles `p:transition`; this is the per-shape
//! animation half.

use thiserror::Error;

/// PresentationML namespace.
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";

/// The animation effect applied to the target shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationEffect {
    /// Fade the shape in (`p:anim` with `effect="fade"`).
    Fade,
    /// Scale/zoom the shape in (`p:anim` with `effect="zoom"`).
    Zoom,
    /// Flip visibility on (a `p:set` to `visible` — a hard appear).
    Appear,
}

impl AnimationEffect {
    fn as_str(self) -> &'static str {
        match self {
            AnimationEffect::Fade => "fade",
            AnimationEffect::Zoom => "zoom",
            AnimationEffect::Appear => "",
        }
    }
}

#[derive(Debug, Error)]
pub enum AnimError {
    #[error("animation effect {0:?} has no p:anim form (use Appear for a p:set)")]
    NoAnimForm(AnimationEffect),
}

/// Build the `<p:timing>…</p:timing>` block that applies `effect` to the shape
/// with `spid` over `duration_ms`. The block is a single-entry main sequence —
/// the minimal valid animation tree PowerPoint accepts. Returns the fragment
/// ready to insert after the slide's `<p:clrMapOvr>` (the schema position of
/// `p:timing`).
pub fn build_timing_xml(
    spid: &str,
    effect: AnimationEffect,
    duration_ms: u32,
) -> Result<String, AnimError> {
    let anim_el = match effect {
        AnimationEffect::Appear => {
            // p:set — flip style.visibility to visible.
            format!(
                "<p:set><p:cBhvr><p:cTn id=\"4\" dur=\"{duration_ms}\" fill=\"hold\">\
                 <p:stCondLst><p:cond delay=\"0\"/></p:stCondLst></p:cTn>\
                 <p:tgtEl><p:spTgt spid=\"{spid}\"/></p:tgtEl>\
                 <p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst>\
                 </p:cBhvr><p:to><p:strVal val=\"visible\"/></p:to></p:set>"
            )
        }
        _ => {
            // p:anim with the effect name (fade / zoom).
            let effect_name = effect.as_str();
            if effect_name.is_empty() {
                return Err(AnimError::NoAnimForm(effect));
            }
            format!(
                "<p:anim effect=\"{effect_name}\"><p:cBhvr>\
                 <p:cTn id=\"4\" dur=\"{duration_ms}\" fill=\"hold\">\
                 <p:stCondLst><p:cond delay=\"0\"/></p:stCondLst></p:cTn>\
                 <p:tgtEl><p:spTgt spid=\"{spid}\"/></p:tgtEl>\
                 </p:cBhvr></p:anim>"
            )
        }
    };
    Ok(format!(
        "<p:timing xmlns:p=\"{P}\"><p:tnLst><p:par><p:cTn id=\"1\" dur=\"indefinite\" restart=\"never\" nodeType=\"tmRoot\">\
         <p:childTnLst><p:seq concurrent=\"1\" nextAc=\"seek\"><p:cTn id=\"2\" dur=\"indefinite\" nodeType=\"mainSeq\">\
         <p:childTnLst><p:par><p:cTn id=\"3\" fill=\"hold\">\
         <p:stCondLst><p:cond delay=\"indefinite\"/></p:stCondLst>\
         <p:childTnLst>{anim_el}</p:childTnLst>\
         </p:cTn></p:par></p:childTnLst></p:cTn></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_builds_anim_with_effect() {
        let xml = build_timing_xml("2", AnimationEffect::Fade, 500).unwrap();
        assert!(xml.contains("<p:anim effect=\"fade\">"), "{xml}");
        assert!(xml.contains("<p:spTgt spid=\"2\"/>"), "{xml}");
        assert!(xml.contains("dur=\"500\""), "{xml}");
        assert!(xml.contains("<p:seq"), "{xml}");
        // Namespace declared on the timing root.
        assert!(xml.starts_with("<p:timing xmlns:p="));
        // Re-parses as XML.
        assert!(roxmltree::Document::parse(&xml).is_ok());
    }

    #[test]
    fn zoom_builds_anim_with_effect() {
        let xml = build_timing_xml("7", AnimationEffect::Zoom, 800).unwrap();
        assert!(xml.contains("<p:anim effect=\"zoom\">"), "{xml}");
        assert!(xml.contains("<p:spTgt spid=\"7\"/>"), "{xml}");
    }

    #[test]
    fn appear_builds_set_to_visible() {
        let xml = build_timing_xml("3", AnimationEffect::Appear, 100).unwrap();
        assert!(xml.contains("<p:set>"), "{xml}");
        assert!(
            xml.contains("<p:attrName>style.visibility</p:attrName>"),
            "{xml}"
        );
        assert!(xml.contains("<p:strVal val=\"visible\"/>"), "{xml}");
        assert!(!xml.contains("<p:anim effect"), "{xml}");
    }
}
