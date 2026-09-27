//! P2.9 — behavioral realism (E14): humanized input generation.
//!
//! ARCH/08 §8.10 step 1 (CloakBrowser/Fortress `humanize=True` pattern):
//! humanized input on `act` — cubic Bézier mouse curves with natural click
//! targets, and per-key typing cadence with natural variance. Optional
//! **per-site** (some sites need it, most don't), off by default so the
//! deterministic default engine is unchanged.
//!
//! Everything is deterministic when a seed is provided, so tests drive the
//! path/cadence generators with a fixed seed.

use crate::actions::Point;
use std::time::Duration;

/// Seeded xorshift64 — deterministic jitter for tests, fast in prod.
#[derive(Debug, Clone)]
pub struct XorShift(pub u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

/// Mouse-movement profile (Bézier curves + natural click targets).
#[derive(Debug, Clone)]
pub struct MouseProfile {
    pub enabled: bool,
    /// Extra jitter (px) applied to the endpoint — "natural click targets".
    pub click_jitter_px: f64,
    /// Control-point spread perpendicular to the chord (px).
    pub curve_spread: f64,
    /// Min/max number of `mouseMoved` steps (scaled by distance).
    pub steps: (u32, u32),
    /// Delay between `mouseMoved` events (ms).
    pub move_delay_ms: (u64, u64),
}

impl Default for MouseProfile {
    fn default() -> Self {
        Self {
            enabled: false,
            click_jitter_px: 1.5,
            curve_spread: 40.0,
            steps: (8, 20),
            move_delay_ms: (5, 18),
        }
    }
}

impl MouseProfile {
    pub fn human() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }
}

/// Typing-cadence profile (per-key delays with natural variance).
#[derive(Debug, Clone)]
pub struct TypingProfile {
    pub enabled: bool,
    /// Typing speed in chars per minute (mean).
    pub cpm: f64,
    /// Relative per-char variance (0..1).
    pub variance: f64,
    /// Extra pause (ms) after word boundaries (whitespace / punctuation).
    pub word_pause_ms: u64,
}

impl Default for TypingProfile {
    fn default() -> Self {
        Self {
            enabled: false,
            cpm: 360.0,
            variance: 0.35,
            word_pause_ms: 60,
        }
    }
}

impl TypingProfile {
    pub fn human() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }
}

/// The whole per-site behavior profile.
#[derive(Debug, Clone, Default)]
pub struct BehaviorProfile {
    pub mouse: MouseProfile,
    pub typing: TypingProfile,
    /// Hosts where behavior is enabled. Empty = all sites (when enabled).
    pub sites: Vec<String>,
    /// RNG seed — `Some` for deterministic tests, `None` for time-seeded prod.
    pub seed: Option<u64>,
}

impl BehaviorProfile {
    /// Humanized everywhere (all sites), time-seeded.
    pub fn human() -> Self {
        Self {
            mouse: MouseProfile::human(),
            typing: TypingProfile::human(),
            ..Self::default()
        }
    }

    /// Restrict humanization to the given hosts (case-insensitive, port
    /// stripped). Empty list = all sites.
    pub fn for_sites(mut self, hosts: &[&str]) -> Self {
        self.sites = hosts.iter().map(|h| h.to_lowercase()).collect();
        self
    }

    /// Deterministic seed for tests.
    pub fn seeded(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Whether humanization applies to the current page URL.
    pub fn site_enabled(&self, url: &str) -> bool {
        if !self.mouse.enabled && !self.typing.enabled {
            return false;
        }
        if self.sites.is_empty() {
            return true;
        }
        match host_of(url) {
            Some(h) => self.sites.iter().any(|s| s == &h),
            None => false,
        }
    }
}

/// Lowercased host (scheme + port + path stripped) of a URL, best-effort.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

/// Cubic Bézier point at `t` (0..=1).
pub fn bezier(t: f64, p0: &Point, p1: &Point, p2: &Point, p3: &Point) -> Point {
    let u = 1.0 - t;
    Point {
        x: u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x,
        y: u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y,
    }
}

/// Humanized mouse path `from` → `to`: cubic Bézier whose control points are
/// offset perpendicular to the chord by `curve_spread`, endpoint jittered by
/// `click_jitter_px`, step count scaled by distance. Returns the interior
/// move points (the caller emits `mouseMoved` for each).
pub fn mouse_path(
    rng: &mut XorShift,
    profile: &MouseProfile,
    from: &Point,
    to: &Point,
) -> Vec<Point> {
    let dist = ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt();
    if dist <= 0.0 {
        return Vec::new();
    }
    let steps = (dist / 40.0)
        .clamp(profile.steps.0 as f64, profile.steps.1 as f64)
        .round() as u32;
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = dist.max(1e-9);
    let (nx, ny) = (-dy / len, dx / len); // unit normal to the chord
    let s1 = rng.range(0.25, 0.5);
    let s2 = rng.range(0.5, 0.75);
    let w1 = rng.range(-1.0, 1.0) * profile.curve_spread;
    let w2 = rng.range(-1.0, 1.0) * profile.curve_spread;
    let p1 = Point {
        x: from.x + dx * s1 + nx * w1,
        y: from.y + dy * s1 + ny * w1,
    };
    let p2 = Point {
        x: from.x + dx * s2 + nx * w2,
        y: from.y + dy * s2 + ny * w2,
    };
    let jx = rng.range(-profile.click_jitter_px, profile.click_jitter_px);
    let jy = rng.range(-profile.click_jitter_px, profile.click_jitter_px);
    let end = Point {
        x: to.x + jx,
        y: to.y + jy,
    };
    (1..=steps)
        .map(|i| bezier(i as f64 / steps as f64, from, &p1, &p2, &end))
        .collect()
}

/// Per-character typing delays (one per `char`) with natural variance; word
/// boundaries get an extra pause.
pub fn typing_delays(rng: &mut XorShift, profile: &TypingProfile, text: &str) -> Vec<Duration> {
    let mean_ms = 60_000.0 / profile.cpm.max(1.0);
    text.chars()
        .map(|c| {
            let factor = 1.0 + rng.range(-profile.variance, profile.variance);
            let mut ms = mean_ms * factor.max(0.15);
            if c.is_whitespace() || c.is_ascii_punctuation() {
                ms += profile.word_pause_ms as f64;
            }
            Duration::from_millis(ms.max(1.0) as u64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn xorshift_is_deterministic() {
        let mut a = XorShift::new(42);
        let mut b = XorShift::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn bezier_interpolates_endpoints() {
        let p0 = pt(0.0, 0.0);
        let p1 = pt(25.0, 10.0);
        let p2 = pt(75.0, -10.0);
        let p3 = pt(100.0, 0.0);
        assert_eq!(bezier(0.0, &p0, &p1, &p2, &p3), p0);
        assert_eq!(bezier(1.0, &p0, &p1, &p2, &p3), p3);
        let mid = bezier(0.5, &p0, &p1, &p2, &p3);
        assert!(mid.x > 25.0 && mid.x < 75.0); // travels along the chord
    }

    #[test]
    fn mouse_path_is_jittered_steps_scaled_and_deterministic() {
        let mut rng = XorShift::new(7);
        let prof = MouseProfile::human();
        let from = pt(100.0, 100.0);
        let to = pt(400.0, 300.0);
        let path = mouse_path(&mut rng, &prof, &from, &to);
        // dist ≈ 360 → steps ≈ 9 (360/40), within (8, 20).
        assert!(
            (8..=20).contains(&(path.len() as u32)),
            "steps={}",
            path.len()
        );
        let last = path.last().unwrap();
        assert!((last.x - to.x).abs() <= prof.click_jitter_px + 0.001);
        assert!((last.y - to.y).abs() <= prof.click_jitter_px + 0.001);
        // Determinism: same seed → same path.
        let mut rng2 = XorShift::new(7);
        assert_eq!(path, mouse_path(&mut rng2, &prof, &from, &to));
    }

    #[test]
    fn mouse_path_zero_distance_is_empty() {
        let mut rng = XorShift::new(1);
        assert!(
            mouse_path(
                &mut rng,
                &MouseProfile::human(),
                &pt(5.0, 5.0),
                &pt(5.0, 5.0)
            )
            .is_empty()
        );
    }

    #[test]
    fn typing_delays_positive_with_word_pause_and_variance() {
        let mut rng = XorShift::new(3);
        let prof = TypingProfile::human();
        let d = typing_delays(&mut rng, &prof, "hi there");
        assert_eq!(d.len(), 8);
        for dur in &d {
            assert!(dur.as_millis() >= 1);
        }
        // The space (index 2) must carry an extra word pause over the mean.
        let mean = 60_000.0 / prof.cpm; // ≈166ms
        assert!(
            d[2].as_millis() as f64 > mean + prof.word_pause_ms as f64 - 1.0,
            "space delay {:?} should exceed mean {mean} + pause",
            d[2]
        );
        // Natural variance: delays are not all identical.
        assert!(d.iter().any(|x| x != &d[0]));
    }

    #[test]
    fn site_enabled_gating() {
        // Off by default (both profiles disabled).
        assert!(!BehaviorProfile::default().site_enabled("https://example.com"));
        // Enabled + empty sites → everywhere.
        assert!(BehaviorProfile::human().site_enabled("https://example.com"));
        assert!(BehaviorProfile::human().site_enabled("https://other.org/"));
        // Enabled + sites → only matching hosts, case/port/path insensitive.
        let b = BehaviorProfile::human().for_sites(&["Example.COM"]);
        assert!(b.site_enabled("https://example.com:8443/login?x=1"));
        assert!(!b.site_enabled("https://other.org"));
        assert!(!b.site_enabled(""));
        // Mouse-only or typing-only still gates.
        let b = BehaviorProfile {
            typing: TypingProfile::default(),
            ..BehaviorProfile::human()
        };
        assert!(b.site_enabled("https://example.com"));
    }

    #[test]
    fn host_of_strips_scheme_port_path() {
        assert_eq!(
            host_of("https://Sub.Example.com:8080/a/b?q=1#frag"),
            Some("sub.example.com".into())
        );
        assert_eq!(host_of("example.com"), Some("example.com".into()));
        assert_eq!(host_of(""), None);
        assert_eq!(host_of("file:///tmp/x"), None);
    }
}
