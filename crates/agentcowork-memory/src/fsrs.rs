//! FSRS-6 spaced-repetition scheduler (C13 — doc 63 §2.2).
//!
//! A faithful Rust port of `open-spaced-repetition/fsrs-rs` (`model.rs` /
//! `inference.rs`). Anki's AGPL-licensed `rslib` is not a dependency; the
//! implementation follows the permissively licensed fsrs-rs model instead. The memory state is `(stability S, difficulty D)`; a
//! review rating is Again(1) / Hard(2) / Good(3) / Easy(4).
//!
//! The core question the scheduler answers is "when should I next review
//! this fact so its recall probability stays at my desired retention?" —
//! used here for the "reinforce what I learned" flow: post-session
//! candidate extraction → FSRS queue → review prompts at optimal intervals.
//!
//! - `Fsrs` — the 21-parameter model (`DEFAULT_PARAMETERS`), with
//!   0/17/19/21-param construction (FSRS-4.5/FSRS-5/FSRS-6 conversion).
//! - `MemoryState`, `Rating`, `NextStates`, `ItemState` — the state model.
//! - `power_forgetting_curve`, `current_retrievability`, `next_interval`,
//!   `next_states` — the prediction surface.
//! - `simulate` — a deterministic workload simulator for eval/retention
//!   metrics (the "does this deck blow up the review queue?" question).

/// Default decay for FSRS-5 (19 params).
pub const FSRS5_DEFAULT_DECAY: f32 = 0.5;
/// Default decay for FSRS-6 (21 params).
pub const FSRS6_DEFAULT_DECAY: f32 = 0.1542;

/// The default parameters — fit to the average person's learning habits.
pub const DEFAULT_PARAMETERS: [f32; 21] = [
    0.212,
    1.2931,
    2.3065,
    8.2956,
    6.4133,
    0.8334,
    3.0194,
    0.001,
    1.8722,
    0.1666,
    0.796,
    1.4835,
    0.0614,
    0.2629,
    1.6483,
    0.6014,
    1.8729,
    0.5425,
    0.0912,
    0.0658,
    FSRS6_DEFAULT_DECAY,
];

/// Clamp bounds (fsrs-rs `simulation.rs`).
const S_MIN: f32 = 0.001;
const S_MAX: f32 = 36_500.0;
const D_MIN: f32 = 1.0;
const D_MAX: f32 = 10.0;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FsrsError {
    #[error("invalid FSRS parameter count: expected 0, 17, 19, or 21, got {0}")]
    InvalidParameterCount(usize),
    #[error("FSRS parameters must be finite")]
    NonFiniteParameter,
}

/// The memory state of one reviewable item: stability (days at R=90%) and
/// difficulty (1..=10).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MemoryState {
    pub stability: f32,
    pub difficulty: f32,
}

impl MemoryState {
    pub const fn new(stability: f32, difficulty: f32) -> Self {
        Self {
            stability,
            difficulty,
        }
    }
}

/// A review rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    pub const fn as_f32(self) -> f32 {
        self as u32 as f32
    }

    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// One answer button's outcome: the resulting memory state + next interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemState {
    pub memory: MemoryState,
    pub interval: f32,
}

/// The four answer buttons for a single review.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NextStates {
    pub again: ItemState,
    pub hard: ItemState,
    pub good: ItemState,
    pub easy: ItemState,
}

/// The FSRS model. `parameters` is always 21 wide internally; construction
/// accepts 0 / 17 / 19 / 21 and converts (see `check_and_fill_parameters`).
#[derive(Debug, Clone)]
pub struct Fsrs {
    parameters: [f32; 21],
}

impl Default for Fsrs {
    fn default() -> Self {
        Self::new(&[]).expect("default parameters are valid")
    }
}

impl Fsrs {
    /// Build from 0 / 17 / 19 / 21 parameters. An empty slice uses the
    /// defaults. 17-param (FSRS-4.5) and 19-param (FSRS-5) inputs are
    /// converted to the 21-param (FSRS-6) space exactly as fsrs-rs does.
    pub fn new(parameters: &[f32]) -> Result<Self, FsrsError> {
        let filled = check_and_fill_parameters(parameters)?;
        let mut arr = [0.0f32; 21];
        arr.copy_from_slice(&filled);
        Ok(Self { parameters: arr })
    }

    pub const fn parameters(&self) -> &[f32; 21] {
        &self.parameters
    }

    /// The (positive) decay `-w20` of the power forgetting curve.
    pub fn decay(&self) -> f32 {
        -self.parameters[20]
    }

    /// Probability of recall after `days_elapsed` given `state`.
    pub fn current_retrievability(&self, state: MemoryState, days_elapsed: f32) -> f32 {
        power_forgetting_curve(&self.parameters, days_elapsed, state.stability)
    }

    /// Next interval for `stability` at `desired_retention`. Pass
    /// `stability = None` for a brand-new card (initial stability from
    /// `rating` is used).
    pub fn next_interval(
        &self,
        stability: Option<f32>,
        desired_retention: f32,
        rating: Rating,
    ) -> f32 {
        let stability = stability.unwrap_or_else(|| self.init_stability(rating));
        self.next_interval_for_stability(stability, desired_retention)
    }

    /// The four answer buttons for one review. `current_memory_state = None`
    /// for a new card (first review). `days_elapsed` is days since last
    /// review (0 for a same-day review).
    pub fn next_states(
        &self,
        current_memory_state: Option<MemoryState>,
        desired_retention: f32,
        days_elapsed: u32,
    ) -> NextStates {
        let (state, nth) = match current_memory_state {
            Some(s) => (s, 1usize),
            None => (MemoryState::new(0.0, 0.0), 0usize),
        };
        let mut next = (1..=4).map(|rating| {
            let memory = validate_state(self.step(days_elapsed as f32, rating, state, nth));
            let interval = self.next_interval_for_stability(memory.stability, desired_retention);
            ItemState { memory, interval }
        });
        NextStates {
            again: next.next().unwrap(),
            hard: next.next().unwrap(),
            good: next.next().unwrap(),
            easy: next.next().unwrap(),
        }
    }

    fn next_interval_for_stability(&self, stability: f32, desired_retention: f32) -> f32 {
        next_interval(&self.parameters, stability, desired_retention)
    }

    fn init_stability(&self, rating: Rating) -> f32 {
        init_stability(&self.parameters, rating.as_u32() as usize)
    }

    /// One review step: `delta_t` days since last review, `rating` answer.
    fn step(&self, delta_t: f32, rating: u32, state: MemoryState, nth: usize) -> MemoryState {
        step(&self.parameters, delta_t, rating as f32, state, nth)
    }
}

// ---------------------------------------------------------------------------
// Model math (private; fsrs-rs `model.rs` — 1:1 formulas)
// ---------------------------------------------------------------------------

#[inline]
fn power_forgetting_curve(w: &[f32; 21], t: f32, s: f32) -> f32 {
    let decay = -w[20];
    let factor = (0.9f32.ln() / decay).exp() - 1.0;
    (t / s * factor + 1.0).powf(decay)
}

#[inline]
fn next_interval(w: &[f32; 21], stability: f32, desired_retention: f32) -> f32 {
    let decay = -w[20];
    let factor = (0.9f32.ln() / decay).exp() - 1.0;
    stability / factor * (desired_retention.powf(1.0 / decay) - 1.0)
}

#[inline]
fn init_stability(w: &[f32; 21], rating: usize) -> f32 {
    w[rating.saturating_sub(1).min(3)]
}

#[inline]
fn init_difficulty(w: &[f32; 21], rating: usize) -> f32 {
    w[4] - (w[5] * rating.saturating_sub(1) as f32).exp() + 1.0
}

#[inline]
fn mean_reversion(w: &[f32; 21], new_d: f32) -> f32 {
    w[7] * (init_difficulty(w, 4) - new_d) + new_d
}

#[inline]
fn linear_damping(delta_d: f32, old_d: f32) -> f32 {
    (10.0 - old_d) * delta_d / 9.0
}

#[inline]
fn next_difficulty(w: &[f32; 21], difficulty: f32, rating: f32) -> f32 {
    let delta_d = -w[6] * (rating - 3.0);
    difficulty + linear_damping(delta_d, difficulty)
}

#[inline]
fn stability_after_success(w: &[f32; 21], last_s: f32, last_d: f32, r: f32, rating: f32) -> f32 {
    let hard_penalty = if rating == 2.0 { w[15] } else { 1.0 };
    let easy_bonus = if rating == 4.0 { w[16] } else { 1.0 };
    last_s
        * (w[8].exp()
            * (11.0 - last_d)
            * last_s.powf(-w[9])
            * (((1.0 - r) * w[10]).exp() - 1.0)
            * hard_penalty
            * easy_bonus
            + 1.0)
}

#[inline]
fn stability_after_failure(w: &[f32; 21], last_s: f32, last_d: f32, r: f32) -> f32 {
    let new_s = w[11]
        * last_d.powf(-w[12])
        * ((last_s + 1.0).powf(w[13]) - 1.0)
        * ((1.0 - r) * w[14]).exp();
    let new_s_min = last_s / (w[17] * w[18]).exp();
    new_s.min(new_s_min)
}

#[inline]
fn stability_short_term(w: &[f32; 21], last_s: f32, rating: f32) -> f32 {
    let sinc = (w[17] * (rating - 3.0 + w[18])).exp() * last_s.powf(-w[19]);
    last_s * if rating >= 2.0 { sinc.max(1.0) } else { sinc }
}

#[inline]
fn step(w: &[f32; 21], delta_t: f32, rating: f32, state: MemoryState, nth: usize) -> MemoryState {
    let last_s = state.stability.clamp(S_MIN, S_MAX);
    let last_d = state.difficulty.clamp(D_MIN, D_MAX);
    let retrievability = power_forgetting_curve(w, delta_t, last_s);
    let stability_after_success =
        stability_after_success(w, last_s, last_d, retrievability, rating);
    let stability_after_failure = stability_after_failure(w, last_s, last_d, retrievability);
    let stability_short_term = stability_short_term(w, last_s, rating);
    let mut new_s = if rating == 1.0 {
        stability_after_failure
    } else {
        stability_after_success
    };
    if delta_t == 0.0 {
        new_s = stability_short_term;
    }
    let mut new_d = next_difficulty(w, last_d, rating);
    new_d = mean_reversion(w, new_d).clamp(D_MIN, D_MAX);
    if nth == 0 && state.stability == 0.0 {
        let init_rating = (rating as u32).clamp(1, 4) as usize;
        new_s = init_stability(w, init_rating);
        new_d = init_difficulty(w, init_rating).clamp(D_MIN, D_MAX);
    }
    if rating == 0.0 {
        new_s = last_s;
        new_d = last_d;
    }
    MemoryState {
        stability: new_s.clamp(S_MIN, S_MAX),
        difficulty: new_d,
    }
}

fn validate_state(state: MemoryState) -> MemoryState {
    MemoryState {
        stability: state.stability.clamp(S_MIN, S_MAX),
        difficulty: state.difficulty.clamp(D_MIN, D_MAX),
    }
}

/// Fill/convert the parameter slice into 21 wide (fsrs-rs
/// `check_and_fill_parameters`).
fn check_and_fill_parameters(parameters: &[f32]) -> Result<Vec<f32>, FsrsError> {
    let parameters = match parameters.len() {
        0 => DEFAULT_PARAMETERS.to_vec(),
        17 => {
            let mut p = parameters.to_vec();
            p[4] = p[5].mul_add(2.0, p[4]);
            p[5] = p[5].mul_add(3.0, 1.0).ln() / 3.0;
            p[6] += 0.5;
            p.extend_from_slice(&[0.0, 0.0, 0.0, FSRS5_DEFAULT_DECAY]);
            p
        }
        19 => {
            let mut p = parameters.to_vec();
            p.extend_from_slice(&[0.0, FSRS5_DEFAULT_DECAY]);
            p
        }
        21 => parameters.to_vec(),
        n => return Err(FsrsError::InvalidParameterCount(n)),
    };
    if parameters.iter().any(|&w| !w.is_finite()) {
        return Err(FsrsError::NonFiniteParameter);
    }
    Ok(parameters)
}

// ---------------------------------------------------------------------------
// Simulator
// ---------------------------------------------------------------------------

/// Workload simulation config: how a deck evolves over time.
#[derive(Debug, Clone, Copy)]
pub struct SimulationConfig {
    /// Target recall probability.
    pub desired_retention: f32,
    /// Number of days to simulate.
    pub days: u32,
    /// New cards introduced each day.
    pub new_cards_per_day: u32,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            desired_retention: 0.9,
            days: 30,
            new_cards_per_day: 10,
        }
    }
}

/// Workload + retention summary of a simulation run.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationReport {
    pub total_reviews: u32,
    /// Reviews scheduled per simulated day.
    pub reviews_per_day: Vec<u32>,
    pub final_card_count: u32,
    pub avg_stability: f32,
    /// Mean retrievability at review time (should track `desired_retention`).
    pub mean_retrievability_at_review: f32,
}

/// One card under simulation: its memory state + when it is due.
#[derive(Debug, Clone, Copy)]
struct SimCard {
    state: MemoryState,
    due_day: u32,
    last_review_day: i32,
}

/// Deterministically simulate `config.days` of reviews. New cards are
/// introduced daily (initial state = a Good first review); each due card is
/// reviewed with a Good rating and rescheduled by the retention target.
pub fn simulate(fsrs: &Fsrs, config: &SimulationConfig) -> SimulationReport {
    let mut cards: Vec<SimCard> = Vec::new();
    let mut reviews_per_day = Vec::with_capacity(config.days as usize);
    let mut total_reviews = 0u32;
    let mut mean_r_sum = 0.0f32;
    let mut review_count = 0u32;

    for day in 0..config.days {
        // Introduce new cards (initial Good state).
        for _ in 0..config.new_cards_per_day {
            let state = fsrs
                .next_states(None, config.desired_retention, 0)
                .good
                .memory;
            cards.push(SimCard {
                state,
                due_day: day
                    + fsrs
                        .next_interval(
                            Some(state.stability),
                            config.desired_retention,
                            Rating::Good,
                        )
                        .ceil() as u32,
                last_review_day: day as i32,
            });
        }

        let mut day_reviews = 0u32;
        for card in cards.iter_mut() {
            if card.due_day > day {
                continue;
            }
            let elapsed = (day as i32 - card.last_review_day).max(0) as f32;
            let r = fsrs.current_retrievability(card.state, elapsed);
            mean_r_sum += r;
            review_count += 1;
            let states =
                fsrs.next_states(Some(card.state), config.desired_retention, elapsed as u32);
            card.state = states.good.memory;
            card.last_review_day = day as i32;
            card.due_day = day
                + fsrs
                    .next_interval(
                        Some(card.state.stability),
                        config.desired_retention,
                        Rating::Good,
                    )
                    .ceil() as u32;
            day_reviews += 1;
        }
        total_reviews += day_reviews;
        reviews_per_day.push(day_reviews);
    }

    let n = cards.len();
    let avg_stability = if n == 0 {
        0.0
    } else {
        cards.iter().map(|c| c.state.stability).sum::<f32>() / n as f32
    };
    let mean_retrievability_at_review = if review_count == 0 {
        0.0
    } else {
        mean_r_sum / review_count as f32
    };

    SimulationReport {
        total_reviews,
        reviews_per_day,
        final_card_count: n as u32,
        avg_stability,
        mean_retrievability_at_review,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parameters_are_21() {
        assert_eq!(DEFAULT_PARAMETERS.len(), 21);
        let fsrs = Fsrs::default();
        assert_eq!(fsrs.parameters(), &DEFAULT_PARAMETERS);
    }

    #[test]
    fn forgetting_curve_matches_published_oracle() {
        // fsrs-rs test_power_forgetting_curve (FSRS-6 defaults).
        let fsrs = Fsrs::default();
        let delta_ts = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let stabilities = [1.0, 2.0, 3.0, 4.0, 4.0, 2.0];
        let expected = [1.0, 0.9403443, 0.9253786, 0.9185229, 0.9, 0.8261359];
        for (((&t, &s), &e), _i) in delta_ts.iter().zip(&stabilities).zip(&expected).zip(0..) {
            let got = fsrs.current_retrievability(MemoryState::new(s, 1.0), t);
            assert!((got - e).abs() < 1e-4, "t={t} s={s}: got {got}, want {e}");
        }
    }

    #[test]
    fn first_review_matches_published_oracle() {
        // fsrs-rs next_states example (new card, elapsed 0, retention 0.9).
        let fsrs = Fsrs::default();
        let next = fsrs.next_states(None, 0.9, 0);
        assert!((next.again.memory.stability - 0.212).abs() < 1e-6);
        assert!((next.again.memory.difficulty - 6.4133).abs() < 1e-4);
        assert!((next.hard.memory.stability - 1.2931).abs() < 1e-6);
        assert!((next.hard.memory.difficulty - 5.1121707).abs() < 1e-5);
        assert!((next.good.memory.stability - 2.3065).abs() < 1e-6);
        assert!((next.good.memory.difficulty - 2.118104).abs() < 1e-5);
        assert!((next.easy.memory.stability - 8.2956).abs() < 1e-6);
        assert!((next.easy.memory.difficulty - 1.0).abs() < 1e-4);
    }

    #[test]
    fn interval_hits_desired_retention() {
        // next_interval must return the t where R(t,S) == desired retention.
        let fsrs = Fsrs::default();
        for (s, r) in [(1.0f32, 0.9f32), (5.0, 0.8), (12.0, 0.95), (100.0, 0.6)] {
            let state = MemoryState::new(s, 5.0);
            let interval = fsrs.next_interval(Some(s), r, Rating::Good);
            let back = fsrs.current_retrievability(state, interval);
            assert!(
                (back - r).abs() < 1e-3,
                "s={s} r={r}: interval {interval} → R {back}"
            );
        }
    }

    #[test]
    fn param_conversion_4dot5_to_6() {
        // fsrs-rs test_convert_parameters: FSRS-4.5 params → 21 wide.
        let fsrs4dot5 = [
            0.4, 0.6, 2.4, 5.8, 4.93, 0.94, 0.86, 0.01, 1.49, 0.14, 0.94, 2.18, 0.05, 0.34, 1.26,
            0.29, 2.61,
        ];
        let fsrs = Fsrs::new(&fsrs4dot5).unwrap();
        let p = fsrs.parameters();
        let expected: Vec<f32> = vec![
            0.4, 0.6, 2.4, 5.8, 6.81, 0.44675013, 1.36, 0.01, 1.49, 0.14, 0.94, 2.18, 0.05, 0.34,
            1.26, 0.29, 2.61, 0.0, 0.0, 0.0, 0.5,
        ];
        for (i, (got, want)) in p.iter().zip(&expected).enumerate() {
            assert!(
                (got - want).abs() < 1e-5,
                "param[{i}]: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn param_conversion_5_to_6_and_invalid() {
        let fsrs5 = DEFAULT_PARAMETERS[..19].to_vec();
        let fsrs = Fsrs::new(&fsrs5).unwrap();
        assert_eq!(fsrs.parameters()[19], 0.0);
        assert_eq!(fsrs.parameters()[20], FSRS5_DEFAULT_DECAY);

        assert!(matches!(
            Fsrs::new(&[1.0, 2.0]),
            Err(FsrsError::InvalidParameterCount(2))
        ));
        let mut bad = [0.0f32; 21];
        bad[0] = f32::NAN;
        assert!(matches!(
            Fsrs::new(&bad),
            Err(FsrsError::NonFiniteParameter)
        ));
    }

    #[test]
    fn failure_lowers_stability_and_clamps_difficulty() {
        let fsrs = Fsrs::default();
        // A high-stability card, lapsed (Again) — stability must drop.
        let high = MemoryState::new(100.0, 3.0);
        let next = fsrs.next_states(Some(high), 0.9, 10).again;
        assert!(next.memory.stability < high.stability);
        assert!((1.0..=10.0).contains(&next.memory.difficulty));
    }

    #[test]
    fn simulator_converges_on_retention_target() {
        let fsrs = Fsrs::default();
        let report = simulate(
            &fsrs,
            &SimulationConfig {
                desired_retention: 0.9,
                days: 60,
                new_cards_per_day: 10,
            },
        );
        assert!(report.total_reviews > 0);
        assert_eq!(report.reviews_per_day.len(), 60);
        assert_eq!(report.final_card_count, 600);
        // The achieved mean retrievability at review time should track the
        // target (within a loose band — Good-only reviews keep it ≥ target).
        assert!(
            report.mean_retrievability_at_review >= 0.85,
            "mean R at review {}",
            report.mean_retrievability_at_review
        );
        // A higher retention target → more reviews (shorter intervals).
        let lax = simulate(
            &fsrs,
            &SimulationConfig {
                desired_retention: 0.7,
                days: 60,
                new_cards_per_day: 10,
            },
        );
        assert!(
            lax.total_reviews < report.total_reviews,
            "0.7 retention should need fewer reviews than 0.9"
        );
    }

    #[test]
    fn stability_grows_across_good_reviews() {
        let fsrs = Fsrs::default();
        let mut state = fsrs.next_states(None, 0.9, 0).good.memory;
        let s0 = state.stability;
        for _ in 0..5 {
            let interval = fsrs
                .next_interval(Some(state.stability), 0.9, Rating::Good)
                .ceil() as u32;
            state = fsrs.next_states(Some(state), 0.9, interval).good.memory;
        }
        assert!(
            state.stability > s0,
            "stability should grow with good reviews"
        );
    }
}
