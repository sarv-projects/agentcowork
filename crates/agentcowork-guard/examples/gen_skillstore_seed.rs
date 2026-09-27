//! One-off tool for the skills-store operator: generates a fresh signing key
//! + signed seed index, printing the public key b64 and the signed index JSON
//!   that the *app* embeds (the app ships only the verifying key + signed
//!   index; the private signing key never leaves the operator's tooling).
//!
//!   cargo run -p agentcowork-guard --example gen_skillstore_seed --release
//!
//! The printed `PUBLIC_KEY_B64` goes into the shell's pinned key const and the
//! `SIGNED_INDEX_JSON` into the bundled index const. Re-run for a fresh pair.

use agentcowork_guard::skillstore::{SkillRow, sign_skill_index};

fn row(id: &str, name: &str, version: &str, description: &str, permissions: &[&str]) -> SkillRow {
    SkillRow {
        id: id.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        description: description.to_string(),
        permissions: permissions.iter().map(|s| s.to_string()).collect(),
        manifest: id.to_string(),
    }
}

fn main() {
    let keys = agentcowork_guard::skillstore::generate_store_keys();

    let rows = vec![
        row(
            "docx-assistant",
            "DOCX Assistant",
            "1.2.0",
            "Draft and format .docx documents from plain instructions.",
            &["fs.write", "tool.mcp"],
        ),
        row(
            "note-taker",
            "Note Taker",
            "0.9.0",
            "Read your notes and surface the ones relevant to the current task.",
            &["fs.read"],
        ),
        row(
            "doc-scanner",
            "Document Scanner",
            "0.4.1",
            "Scan a folder for documents and summarize contents.",
            &["fs.read", "tool.mcp"],
        ),
        row(
            "email-drafter",
            "Email Drafter",
            "1.0.0",
            "Draft replies in your tone from an inbox thread.",
            &["fs.read", "tool.connector"],
        ),
    ];

    let signed = sign_skill_index(&rows, &keys.signing);
    println!("PUBLIC_KEY_B64:");
    println!("{}", keys.public_key_b64);
    println!("SIGNED_INDEX_JSON:");
    println!("{}", signed.body);
    println!("SIGNATURE_B64:");
    println!("{}", signed.signature_b64);
}
