//! Live BYOK streaming round-trip against NVIDIA NIM (P1.2, last task).
//!
//! The API key is read from `NVIDIA_NIM_API_KEY` **at runtime only** — it is
//! never committed, never logged, and stored only inside a throwaway
//! SQLCipher vault in the temp dir (encrypted at rest).
//!
//! ```sh
//! NVIDIA_NIM_API_KEY=nvapi-... cargo run -p agentcowork-vault --example nim_stream
//! ```
//!
//! Demonstrates the full P1.1/P1.2 flow: key ingest → opaque handle →
//! broker key selection → auth injection → SSE stream → health/usage recorded.

use agentcowork_vault::{Broker, KeyRing, KeySpec, KeyStatus};
use zeroize::Zeroize;

fn main() {
    let mut key = std::env::var("NVIDIA_NIM_API_KEY").expect("set NVIDIA_NIM_API_KEY");
    if key.trim().is_empty() {
        eprintln!("NVIDIA_NIM_API_KEY is empty");
        std::process::exit(2);
    }
    let model = std::env::var("NIM_MODEL").unwrap_or_else(|_| "meta/llama-3.3-70b-instruct".into());

    // Throwaway encrypted vault in the temp dir (not the repo).
    let dir = std::env::temp_dir().join("agentcowork-nim-live");
    let path = dir.join("vault.db");
    let _ = std::fs::remove_dir_all(&dir);
    let vault = agentcowork_vault::Vault::open(&path, "live-test-key-do-not-use").expect("vault");

    // P1.1: ingest the key → opaque handle minted; the secret goes to the
    // SQLCipher row. Scrub our env copy immediately after.
    let ring = KeyRing::new(&vault);
    let handle = ring
        .add_key(KeySpec {
            provider: "nvidia".into(),
            key_id: "nim-live".into(),
            value: key.as_bytes().to_vec(),
            status: KeyStatus::Primary,
            model_filter: vec![],
            priority: 100,
            daily_token_cap: None,
            daily_cost_cap: None,
        })
        .expect("ingest key");
    key.zeroize();
    println!("key ingested, opaque handle = {handle} (secret zeroized from env)");

    // P1.2: broker streaming round-trip.
    let broker = Broker::new(&vault);
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": "You are terse." },
            { "role": "user", "content": "Reply with exactly: AgentCowork broker round-trip OK" }
        ],
        "max_tokens": 64,
        "temperature": 0.0,
    });
    let events = broker
        .chat_completion_stream("nvidia", &model, "live-session-1", body)
        .expect("stream round-trip");
    let text: String = events.iter().filter_map(|e| e.delta.clone()).collect();
    let finished = events.iter().any(|e| e.finish.is_some());
    println!("NIM response ({finished}): {text}");
    assert!(!text.trim().is_empty(), "NIM returned no content");

    // Health/usage landed on the ring.
    let info = ring.list("nvidia").expect("list");
    let k = info.iter().find(|i| i.key_id == "nim-live").expect("key");
    println!(
        "ring: success_count={} fail_count={} tokens_day={}",
        k.success_count, k.fail_count, k.tokens_day
    );
    println!("LIVE ROUND-TRIP OK");
    let _ = std::fs::remove_dir_all(&dir);
}
