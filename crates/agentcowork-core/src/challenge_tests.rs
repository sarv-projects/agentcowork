//! P2.8 Challenge Handler tests — detection heuristic, routing (local-PoW vs
//! human-in-loop vs BYO), and the SHA-256 proof-of-work solve/verify round
//! trip. Managed captchas (reCAPTCHA / hCaptcha / Turnstile) must NEVER be
//! claimed locally solvable; PoW kinds must never be paid for.

use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;

#[test]
fn detect_identifies_each_kind_case_insensitively() {
    let h = ChallengeHandler::new();

    assert_eq!(h.detect("altcha-widget"), Some(ChallengeKind::Altcha));
    assert_eq!(
        h.detect("script src=altcha.js /ALTCHA widget"),
        Some(ChallengeKind::Altcha)
    );

    assert_eq!(
        h.detect("Protected by Friendly Captcha (frc-captcha)"),
        Some(ChallengeKind::FriendlyCaptcha)
    );
    assert_eq!(
        h.detect("friendly-challenge widget"),
        Some(ChallengeKind::FriendlyCaptcha)
    );

    assert_eq!(
        h.detect("cf-turnstile challenges.cloudflare.com"),
        Some(ChallengeKind::Turnstile)
    );
    assert_eq!(h.detect("Turnstile"), Some(ChallengeKind::Turnstile));

    assert_eq!(h.detect("hCaptcha"), Some(ChallengeKind::HCaptcha));
    assert_eq!(
        h.detect("h-captcha-response"),
        Some(ChallengeKind::HCaptcha)
    );

    assert_eq!(
        h.detect("g-recaptcha grecaptcha"),
        Some(ChallengeKind::Recaptcha)
    );
    assert_eq!(h.detect("reCAPTCHA v2"), Some(ChallengeKind::Recaptcha));

    assert_eq!(h.detect("a normal login form"), None);
}

#[test]
fn detect_prefers_pow_over_managed_when_both_markers_present() {
    let h = ChallengeHandler::new();
    // A page that embeds Altcha alongside a reCAPTCHA mention must classify
    // as Altcha (locally solvable) — the PoW check runs first.
    let mixed = "sign in — g-recaptcha fallback plus altcha widget";
    assert_eq!(h.detect(mixed), Some(ChallengeKind::Altcha));
}

#[test]
fn route_sends_pow_locally_even_when_byo_configured() {
    let h = ChallengeHandler::new().with_byo_provider("capsolver");

    // PoW is free to solve locally — never pay a solver for it.
    assert_eq!(
        h.route(ChallengeKind::Altcha, "example.com"),
        ChallengeResolution::LocalPow {
            kind: ChallengeKind::Altcha
        }
    );
    assert_eq!(
        h.route(ChallengeKind::FriendlyCaptcha, "example.com"),
        ChallengeResolution::LocalPow {
            kind: ChallengeKind::FriendlyCaptcha
        }
    );
}

#[test]
fn route_managed_captcha_defaults_to_human_in_loop() {
    let h = ChallengeHandler::new();

    assert_eq!(
        h.route(ChallengeKind::Turnstile, "bank.example.com"),
        ChallengeResolution::HumanInLoop {
            site: "bank.example.com".to_string(),
            kind: ChallengeKind::Turnstile,
        }
    );
    assert_eq!(
        h.route(ChallengeKind::Recaptcha, "shop.example.com"),
        ChallengeResolution::HumanInLoop {
            site: "shop.example.com".to_string(),
            kind: ChallengeKind::Recaptcha,
        }
    );
    assert_eq!(
        h.route(ChallengeKind::HCaptcha, "forum.example.com"),
        ChallengeResolution::HumanInLoop {
            site: "forum.example.com".to_string(),
            kind: ChallengeKind::HCaptcha,
        }
    );
}

#[test]
fn route_managed_captcha_to_byo_when_configured() {
    let h = ChallengeHandler::new().with_byo_provider("2captcha");

    assert_eq!(
        h.route(ChallengeKind::Turnstile, "bank.example.com"),
        ChallengeResolution::ByoSolver {
            provider: "2captcha".to_string(),
            kind: ChallengeKind::Turnstile,
        }
    );
    assert_eq!(
        h.route(ChallengeKind::Recaptcha, "shop.example.com"),
        ChallengeResolution::ByoSolver {
            provider: "2captcha".to_string(),
            kind: ChallengeKind::Recaptcha,
        }
    );
}

#[test]
fn solve_pow_finds_valid_nonce_and_verifies() {
    // difficulty 4 = 4 leading zero nibbles (~65k avg tries) — fast but real.
    let salt = "altcha-challenge-123";
    let nonce = ChallengeHandler::solve_pow(salt, 1_000_000, 4);
    let nonce = nonce.expect("a nonce must exist within 1M tries");
    assert!(ChallengeHandler::verify_pow(salt, nonce, 4));

    // The found nonce must NOT satisfy a harder difficulty (it wasn't chosen
    // for it) — verifies the check is actually difficulty-sensitive.
    assert!(!ChallengeHandler::verify_pow(salt, nonce, 8));
}

#[test]
fn verify_pow_rejects_wrong_nonce() {
    let salt = "salt";
    let good = ChallengeHandler::solve_pow(salt, 100_000, 3).unwrap();
    assert!(ChallengeHandler::verify_pow(salt, good, 3));
    assert!(!ChallengeHandler::verify_pow(salt, good + 1, 3));
}

#[test]
fn solve_pow_returns_none_when_no_solution_in_range() {
    // difficulty 8 needs ~2^32 avg tries; a 0..1000 range must fail.
    assert!(ChallengeHandler::solve_pow("altcha", 1000, 8).is_none());
}

#[test]
fn solve_pow_finds_nonce_zero_when_already_valid() {
    // A salt whose sha256 starts with a zero nibble already — solving at
    // difficulty 1 must return quickly and verify.
    let nonce = ChallengeHandler::solve_pow("any-salt", 100_000, 1);
    assert!(nonce.is_some());
    assert!(ChallengeHandler::verify_pow("any-salt", nonce.unwrap(), 1));
}

// --------------------------------------------------------------------------
// human-in-the-loop registry
// --------------------------------------------------------------------------

#[test]
fn surface_registers_and_pending_lists() {
    let h = ChallengeHandler::new();
    let c = h.surface(ChallengeKind::Turnstile, "bank.example.com");
    assert!(c.id.starts_with("hc-"));
    assert_eq!(c.site, "bank.example.com");
    assert_eq!(c.kind, ChallengeKind::Turnstile);
    assert!(h.pending().iter().any(|p| p.id == c.id));
}

#[test]
fn human_challenge_is_single_use() {
    let h = ChallengeHandler::new();
    let c = h.surface(ChallengeKind::Recaptcha, "shop.example.com");
    // First redemption succeeds.
    assert!(h.resolve_human(&c.id).is_some());
    // Second redemption (duplicate/stale solve) is refused.
    assert!(h.resolve_human(&c.id).is_none());
    // Unknown id is refused too.
    assert!(h.resolve_human("hc-nope").is_none());
    assert!(h.pending().is_empty());
}

#[test]
fn resolve_human_removes_from_pending() {
    let h = ChallengeHandler::new();
    let c = h.surface(ChallengeKind::HCaptcha, "forum.example.com");
    assert_eq!(h.pending().len(), 1);
    h.resolve_human(&c.id).unwrap();
    assert!(h.pending().is_empty());
}

// --------------------------------------------------------------------------
// visual grounding
// --------------------------------------------------------------------------

#[test]
fn route_visual_prefers_grounding_for_simple_puzzles() {
    let h = ChallengeHandler::new();
    assert_eq!(
        h.route_visual(ChallengeKind::Recaptcha, "a.com", true),
        ChallengeResolution::VisualGrounding {
            site: "a.com".into(),
            kind: ChallengeKind::Recaptcha
        }
    );
    // When grounding is disallowed, fall back to the normal route (human).
    assert_eq!(
        h.route_visual(ChallengeKind::Recaptcha, "a.com", false),
        ChallengeResolution::HumanInLoop {
            site: "a.com".into(),
            kind: ChallengeKind::Recaptcha
        }
    );
    // PoW kinds are never "grounded" (they solve locally).
    assert_eq!(
        h.route_visual(ChallengeKind::Altcha, "a.com", true),
        ChallengeResolution::LocalPow {
            kind: ChallengeKind::Altcha
        }
    );
}

#[test]
fn grounding_request_wraps_options_and_prompt() {
    let h = ChallengeHandler::new();
    let req = h.grounding_request(
        ChallengeKind::Recaptcha,
        "a.com",
        "pick every tile with a bus",
        vec![
            GroundingOption {
                id: "e1".into(),
                label: "tile (0,0)".into(),
            },
            GroundingOption {
                id: "e2".into(),
                label: "tile (0,1)".into(),
            },
        ],
    );
    assert_eq!(req.site, "a.com");
    assert_eq!(req.options.len(), 2);
    assert_eq!(req.prompt, "pick every tile with a bus");
}

#[test]
fn parse_grounding_choice_option_forms() {
    let ids = vec!["e1".to_string(), "e12".to_string()];
    assert_eq!(
        parse_grounding_choice("e12", &ids),
        Some(GroundingChoice::Option("e12".into()))
    );
    assert_eq!(
        parse_grounding_choice("choose E1", &ids),
        Some(GroundingChoice::Option("e1".into()))
    );
    assert_eq!(
        parse_grounding_choice("the answer is e12", &ids),
        Some(GroundingChoice::Option("e12".into()))
    );
    // A bare token not in the option list is not a valid option.
    assert_eq!(parse_grounding_choice("e99", &ids), None);
}

#[test]
fn parse_grounding_choice_point_forms() {
    let ids: Vec<String> = vec![];
    assert_eq!(
        parse_grounding_choice("(123, 45)", &ids),
        Some(GroundingChoice::Point { x: 123.0, y: 45.0 })
    );
    assert_eq!(
        parse_grounding_choice("click at (640.5, 400)", &ids),
        Some(GroundingChoice::Point { x: 640.5, y: 400.0 })
    );
}

#[test]
fn parse_grounding_choice_unsolvable() {
    let ids: Vec<String> = vec![];
    assert_eq!(
        parse_grounding_choice("I can't tell", &ids),
        Some(GroundingChoice::Unsolvable)
    );
    assert_eq!(
        parse_grounding_choice("unsolvable", &ids),
        Some(GroundingChoice::Unsolvable)
    );
    assert_eq!(
        parse_grounding_choice("", &ids),
        Some(GroundingChoice::Unsolvable)
    );
    assert_eq!(parse_grounding_choice("nonsense text here", &ids), None);
}

// --------------------------------------------------------------------------
// BYO solver HTTP
// --------------------------------------------------------------------------

/// Scripted transport: a queue of (url, body) → response.
struct ScriptedHttp {
    responses: Mutex<Vec<Value>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl ScriptedHttp {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: Mutex::new(responses),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

impl SolverHttp for ScriptedHttp {
    fn post_json(&self, url: &str, body: &Value) -> Result<Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((url.to_string(), body.clone()));
        let mut r = self.responses.lock().unwrap();
        if r.is_empty() {
            return Ok(json!({}));
        }
        Ok(r.remove(0))
    }
}

fn ready_response(task_id: &str, token: &str) -> Value {
    json!({
        "errorId": 0,
        "taskId": task_id,
        "status": "ready",
        "solution": { "gRecaptchaResponse": token }
    })
}

#[test]
fn create_task_posts_to_provider_and_parses_task_id() {
    let http = ScriptedHttp::new(vec![json!({"errorId": 0, "taskId": "t-123"})]);
    let id = create_task(
        &http,
        ByoProvider::CapSolver,
        "key-1",
        "https://example.com",
        "site-key",
    )
    .unwrap();
    assert_eq!(id, "t-123");
    let calls = http.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "https://api.capsolver.com/createTask");
    assert_eq!(calls[0].1["clientKey"], "key-1");
    assert_eq!(calls[0].1["task"]["type"], "ReCaptchaV2TaskProxyLess");
}

#[test]
fn create_task_surfaces_solver_error() {
    let http = ScriptedHttp::new(vec![json!({"errorId": 1, "errorDescription": "bad key"})]);
    let err = create_task(
        &http,
        ByoProvider::TwoCaptcha,
        "key-1",
        "https://example.com",
        "site-key",
    )
    .unwrap_err();
    assert!(matches!(err, ByoSolverError::Solver(1, _)));
}

#[test]
fn poll_task_ready_and_processing() {
    let ready = ScriptedHttp::new(vec![ready_response("t-1", "tok-xyz")]);
    assert_eq!(
        poll_task(&ready, ByoProvider::CapSolver, "k", "t-1").unwrap(),
        Some("tok-xyz".to_string())
    );
    let processing = ScriptedHttp::new(vec![json!({"errorId": 0, "status": "processing"})]);
    assert_eq!(
        poll_task(&processing, ByoProvider::CapSolver, "k", "t-1").unwrap(),
        None
    );
}

#[test]
fn solve_captcha_polls_until_ready() {
    let http = ScriptedHttp::new(vec![
        json!({"errorId": 0, "taskId": "t-9"}),
        json!({"errorId": 0, "status": "processing"}),
        ready_response("t-9", "final-token"),
    ]);
    let token = solve_captcha(
        &http,
        ByoProvider::TwoCaptcha,
        "key",
        "https://example.com",
        "site-key",
        Duration::from_millis(0),
        10,
    )
    .unwrap();
    assert_eq!(token, "final-token");
    // createTask + 2 polls.
    let calls = http.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].0, "https://api.2captcha.com/createTask");
    assert_eq!(calls[1].0, "https://api.2captcha.com/getTaskResult");
    assert_eq!(calls[2].0, "https://api.2captcha.com/getTaskResult");
}

#[test]
fn solve_captcha_times_out_when_never_ready() {
    let http = ScriptedHttp::new(vec![
        json!({"errorId": 0, "taskId": "t-9"}),
        json!({"errorId": 0, "status": "processing"}),
        json!({"errorId": 0, "status": "processing"}),
    ]);
    let err = solve_captcha(
        &http,
        ByoProvider::CapSolver,
        "key",
        "https://example.com",
        "site-key",
        Duration::from_millis(0),
        2,
    )
    .unwrap_err();
    assert!(matches!(err, ByoSolverError::Timeout(2)));
}

#[test]
fn byo_provider_parses() {
    assert_eq!(
        ByoProvider::parse("capsolver"),
        Some(ByoProvider::CapSolver)
    );
    assert_eq!(
        ByoProvider::parse("2captcha"),
        Some(ByoProvider::TwoCaptcha)
    );
    assert_eq!(
        ByoProvider::parse("2CAPTCHA"),
        Some(ByoProvider::TwoCaptcha)
    );
    assert_eq!(ByoProvider::parse("nope"), None);
}
