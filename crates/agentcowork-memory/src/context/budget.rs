//! Budget terms and the **pre-turn feasibility check**
//! (`ARCH/16-CONTEXT.md` §3, `ARCH/15-AGENT-X.md` §5, `REQ-CTX-005`).
//!
//! ```text
//! usable = model_window_resolved
//!        − output_reserve − reasoning_reserve − summary_output_reserve
//!        − tool_schema_reserve − system_reserve − safety_buffer
//! retained_recent = keep
//! ```
//!
//! Every term is a **named, product-visible knob** — there is no magic number in
//! the arithmetic, and the resolved terms are reported so telemetry can show
//! them. Overflow is decided here, **before** the send: a provider-side overflow
//! is a defect, not a recovery path.

use serde::{Deserialize, Serialize};

/// The named budget terms. `output_reserve`, `reasoning_reserve`,
/// `summary_output_reserve`, `tool_schema_reserve` and `system_reserve` are
/// measured per turn; `safety_buffer` and `keep` are the standing knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetTerms {
    /// The resolved window for the selected model (from the model registry,
    /// `ARCH/18-MODEL-ROUTING.md`). The feasibility check uses *this*, never a
    /// hard-coded constant.
    pub model_window_resolved: u32,
    pub output_reserve: u32,
    pub reasoning_reserve: u32,
    pub summary_output_reserve: u32,
    pub tool_schema_reserve: u32,
    pub system_reserve: u32,
    /// The standing safety margin. Default ≈20k, with an absolute floor for
    /// small local models (OQ-CTX-02).
    pub safety_buffer: u32,
    /// The `retained_recent` term: how much recent context survives a
    /// compaction boundary. Default ≈8k.
    pub keep: u32,
}

impl BudgetTerms {
    /// The default envelope for a large cloud model class: buffer/reserve ≈20k,
    /// keep ≈8k (`ARCH/16-CONTEXT.md` §3).
    pub fn cloud(window: u32) -> Self {
        Self {
            model_window_resolved: window,
            output_reserve: 8_192,
            reasoning_reserve: 4_096,
            summary_output_reserve: 2_048,
            tool_schema_reserve: 4_096,
            system_reserve: 2_048,
            safety_buffer: 20_000,
            keep: 8_000,
        }
    }

    /// A small local model: absolute floors, not percent-of-window, because a
    /// percent of a 16k window is not a usable margin.
    pub fn local_small(window: u32) -> Self {
        Self {
            model_window_resolved: window,
            output_reserve: 2_048,
            reasoning_reserve: 0,
            summary_output_reserve: 1_024,
            tool_schema_reserve: 1_024,
            system_reserve: 512,
            safety_buffer: 2_000,
            keep: 2_000,
        }
    }

    /// `usable = window − Σ reserves − buffer`. Saturating at 0 rather than
    /// wrapping: a negative usable window must read as "nothing fits", never as
    /// a huge positive number.
    pub fn usable(&self) -> u32 {
        let reserved = self
            .output_reserve
            .saturating_add(self.reasoning_reserve)
            .saturating_add(self.summary_output_reserve)
            .saturating_add(self.tool_schema_reserve)
            .saturating_add(self.system_reserve)
            .saturating_add(self.safety_buffer);
        self.model_window_resolved.saturating_sub(reserved)
    }

    /// The full term breakdown, for telemetry (`REQ-CTX-005`: "telemetry shows
    /// the named terms").
    pub fn breakdown(&self) -> BudgetBreakdown {
        BudgetBreakdown {
            model_window_resolved: self.model_window_resolved,
            output_reserve: self.output_reserve,
            reasoning_reserve: self.reasoning_reserve,
            summary_output_reserve: self.summary_output_reserve,
            tool_schema_reserve: self.tool_schema_reserve,
            system_reserve: self.system_reserve,
            safety_buffer: self.safety_buffer,
            keep: self.keep,
            reserved_total: self.model_window_resolved.saturating_sub(self.usable()),
            usable: self.usable(),
        }
    }

    /// Whether the terms are internally coherent. A window smaller than the
    /// reserves is a configuration error, not something to discover at send.
    pub fn validate(&self) -> Result<(), BudgetError> {
        if self.usable() == 0 && self.model_window_resolved > 0 {
            return Err(BudgetError::WindowExhaustedByReserves {
                window: self.model_window_resolved,
                reserved: self.model_window_resolved.saturating_sub(self.usable()),
            });
        }
        if self.keep > self.usable() {
            return Err(BudgetError::KeepExceedsUsable {
                keep: self.keep,
                usable: self.usable(),
            });
        }
        Ok(())
    }
}

/// The resolved budget, reported verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetBreakdown {
    pub model_window_resolved: u32,
    pub output_reserve: u32,
    pub reasoning_reserve: u32,
    pub summary_output_reserve: u32,
    pub tool_schema_reserve: u32,
    pub system_reserve: u32,
    pub safety_buffer: u32,
    pub keep: u32,
    pub reserved_total: u32,
    pub usable: u32,
}

/// What a turn actually wants to send, before packing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TurnFootprint {
    pub system_tokens: u32,
    pub message_tokens: u32,
    pub tool_schema_tokens: u32,
}

impl TurnFootprint {
    pub fn total(&self) -> u32 {
        self.system_tokens
            .saturating_add(self.message_tokens)
            .saturating_add(self.tool_schema_tokens)
    }
}

/// The feasibility verdict (`REQ-CTX-005`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Feasibility {
    /// Fits as packed.
    Fits,
    /// Does not fit: enter the compaction path **before** sending.
    NeedsRecovery { over_by: u32 },
    /// Cannot fit even after maximal compaction. Refused before send with
    /// guidance — never sent to fail at the provider.
    Refused {
        reason: RefusalReason,
        shortfall: u32,
    },
}

/// Why a turn is refused outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefusalReason {
    /// The budget terms themselves do not fit the resolved window.
    TermsInvalid,
    /// Even an empty conversation plus the reserved system/tool text exceeds
    /// the usable window.
    MinimumIrreducible,
    /// `keep` alone is larger than the usable window.
    KeepTooLarge,
}

impl Feasibility {
    pub fn is_fits(self) -> bool {
        matches!(self, Feasibility::Fits)
    }
}

/// Budget arithmetic failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    #[error("model window {window} is exhausted by its reserves ({reserved} tokens)")]
    WindowExhaustedByReserves { window: u32, reserved: u32 },
    #[error("keep ({keep}) exceeds the usable window ({usable})")]
    KeepExceedsUsable { keep: u32, usable: u32 },
}

/// The pre-turn feasibility check.
///
/// It runs **before** the send and answers one question: does this turn fit in
/// the usable window? A `NeedsRecovery` verdict is an instruction to the
/// controller (prune → compact), never an error surfaced to the user while
/// recovery options remain (INV: recovery-first).
pub fn check_feasibility(
    terms: &BudgetTerms,
    footprint: &TurnFootprint,
) -> Result<Feasibility, BudgetError> {
    terms.validate()?;
    let usable = terms.usable();
    let total = footprint.total();
    if total <= usable {
        return Ok(Feasibility::Fits);
    }
    // The irreducible floor: the system contract plus the tool schemas cannot
    // be compacted away. If those alone do not fit, no recovery helps.
    let irreducible = footprint
        .system_tokens
        .saturating_add(footprint.tool_schema_tokens);
    if irreducible > usable {
        return Ok(Feasibility::Refused {
            reason: RefusalReason::MinimumIrreducible,
            shortfall: irreducible - usable,
        });
    }
    // `keep` is the retained-recent term: keeping more than fits is a refusal,
    // not an overrun.
    if terms.keep > usable {
        return Ok(Feasibility::Refused {
            reason: RefusalReason::KeepTooLarge,
            shortfall: terms.keep - usable,
        });
    }
    Ok(Feasibility::NeedsRecovery {
        over_by: total - usable,
    })
}

/// The guidance a refusal carries. Refusal is *before* send, with a next step —
/// never a bare error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub reason: RefusalReason,
    pub shortfall: u32,
    pub guidance: Vec<String>,
}

/// Turn a refusal into actionable guidance.
pub fn refusal_guidance(terms: &BudgetTerms, refusal: &Refusal) -> Refusal {
    let mut guidance = Vec::new();
    match refusal.reason {
        RefusalReason::TermsInvalid => guidance.push(
            "The budget terms exceed the resolved model window. Lower a reserve or the safety buffer, or select a model with a larger window."
                .into(),
        ),
        RefusalReason::MinimumIrreducible => guidance.push(format!(
            "The system contract plus tool schemas need {} tokens but only {} are usable. Reduce the tool set, shorten the system contract, or switch to a larger-window model.",
            refusal.shortfall + terms.usable(),
            terms.usable()
        )),
        RefusalReason::KeepTooLarge => guidance.push(format!(
            "The retained-recent term ({} tokens) does not fit the usable window ({} tokens). Lower the keep term or switch to a larger-window model.",
            terms.keep,
            terms.usable()
        )),
    }
    Refusal {
        reason: refusal.reason,
        shortfall: refusal.shortfall,
        guidance,
    }
}

/// The measured token estimate for a piece of text. The documented conservative
/// approximation: 4 characters per token. A tokenizer mismatch must never
/// under-estimate, because that is how a pre-turn check is defeated.
pub fn estimate_tokens(text: &str) -> u32 {
    text.chars().count().div_ceil(4) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_named_terms_subtract_in_the_documented_order() {
        let t = BudgetTerms {
            model_window_resolved: 200_000,
            output_reserve: 8_000,
            reasoning_reserve: 4_000,
            summary_output_reserve: 2_000,
            tool_schema_reserve: 4_000,
            system_reserve: 2_000,
            safety_buffer: 20_000,
            keep: 8_000,
        };
        // 200000 − (8000+4000+2000+4000+2000+20000) = 160000
        assert_eq!(t.usable(), 160_000);
        let b = t.breakdown();
        assert_eq!(b.reserved_total, 40_000);
        assert_eq!(b.keep, 8_000);
        assert_eq!(b.usable, 160_000);
    }

    #[test]
    fn a_small_local_model_class_uses_absolute_floors() {
        let t = BudgetTerms::local_small(16_000);
        assert_eq!(t.safety_buffer, 2_000, "not a percent of a small window");
        // 16_000 − (2_048 output + 1_024 summary + 1_024 tools + 512 system
        // + 2_000 buffer); a small local model asks for no reasoning reserve.
        assert_eq!(t.reasoning_reserve, 0);
        assert_eq!(t.usable(), 16_000 - (2_048 + 1_024 + 1_024 + 512 + 2_000));
        assert!(t.validate().is_ok());
    }

    #[test]
    fn usable_saturates_at_zero_instead_of_wrapping_negative() {
        let t = BudgetTerms {
            model_window_resolved: 1_000,
            output_reserve: 4_000,
            reasoning_reserve: 4_000,
            summary_output_reserve: 4_000,
            tool_schema_reserve: 4_000,
            system_reserve: 4_000,
            safety_buffer: 4_000,
            keep: 0,
        };
        assert_eq!(t.usable(), 0);
        assert!(matches!(
            t.validate(),
            Err(BudgetError::WindowExhaustedByReserves { .. })
        ));
    }

    #[test]
    fn a_turn_that_fits_is_feasible() {
        let t = BudgetTerms::cloud(200_000);
        let f = TurnFootprint {
            system_tokens: 2_000,
            message_tokens: 10_000,
            tool_schema_tokens: 4_000,
        };
        assert_eq!(check_feasibility(&t, &f).unwrap(), Feasibility::Fits);
    }

    #[test]
    fn an_oversized_turn_is_caught_pre_send_with_the_overage() {
        let t = BudgetTerms::cloud(100_000);
        let usable = t.usable();
        let f = TurnFootprint {
            system_tokens: 2_000,
            message_tokens: usable + 500,
            tool_schema_tokens: 4_000,
        };
        // The overage is measured against the *total* footprint, so the 500
        // extra message tokens plus the 6k of system+tools that also sit above
        // the usable window both count.
        assert_eq!(
            check_feasibility(&t, &f).unwrap(),
            Feasibility::NeedsRecovery { over_by: 6_500 }
        );
    }

    #[test]
    fn a_refusal_happens_before_send_and_carries_guidance() {
        // A window so small that the system contract + tools cannot fit.
        // An 8k local window leaves 1,584 usable after the floor reserves; the
        // system contract plus tool schemas need 5,000, so no recovery helps.
        // `keep` is lowered to fit the usable window, because an incoherent
        // term set is a configuration error caught before the turn is measured.
        let t = BudgetTerms {
            keep: 1_000,
            ..BudgetTerms::local_small(8_192)
        };
        let f = TurnFootprint {
            system_tokens: 3_000,
            message_tokens: 0,
            tool_schema_tokens: 2_000,
        };
        let v = check_feasibility(&t, &f).unwrap();
        let Feasibility::Refused { reason, shortfall } = v else {
            panic!("expected a refusal, got {v:?}")
        };
        assert_eq!(reason, RefusalReason::MinimumIrreducible);
        let g = refusal_guidance(
            &t,
            &Refusal {
                reason,
                shortfall,
                guidance: Vec::new(),
            },
        );
        assert!(!g.guidance.is_empty());
        assert!(g.guidance[0].contains("larger-window model"));
    }

    #[test]
    fn a_keep_larger_than_the_usable_window_is_refused() {
        let t = BudgetTerms {
            model_window_resolved: 20_000,
            output_reserve: 2_000,
            reasoning_reserve: 0,
            summary_output_reserve: 1_000,
            tool_schema_reserve: 1_000,
            system_reserve: 500,
            safety_buffer: 2_000,
            keep: 20_000,
        };
        let f = TurnFootprint {
            system_tokens: 100,
            message_tokens: 5_000,
            tool_schema_tokens: 100,
        };
        // Terms invalid: keep > usable, so the check errors before it can even
        // measure the turn.
        assert!(matches!(
            check_feasibility(&t, &f),
            Err(BudgetError::KeepExceedsUsable { .. })
        ));
    }

    #[test]
    fn the_estimate_never_under_reports() {
        // Four characters per token, rounded up.
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens(""), 0);
        // A conservative estimate is at least one token per 4 chars, so a long
        // text never reports less than chars/4.
        let long = "word ".repeat(10_000);
        assert!(estimate_tokens(&long) as usize >= long.chars().count() / 4);
    }

    #[test]
    fn the_breakdown_reports_every_named_term() {
        let t = BudgetTerms::cloud(128_000);
        let b = t.breakdown();
        let json = serde_json::to_string(&b).unwrap();
        for term in [
            "model_window_resolved",
            "output_reserve",
            "reasoning_reserve",
            "summary_output_reserve",
            "tool_schema_reserve",
            "system_reserve",
            "safety_buffer",
            "keep",
            "usable",
        ] {
            assert!(json.contains(term), "{term} is missing from telemetry");
        }
    }
}
