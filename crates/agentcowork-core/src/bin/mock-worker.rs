//! P7.9 test fixture — a mock sandboxed worker. Applies its profile flags
//! (`--profile <name> [--scratch <path>]`) at startup (the P7.8 sandbox
//! apply seam — here it just records them), reports `ready <profile>` on
//! stdout, then echoes every request line as `ack:<line>` until `bye`.

use std::io::{BufRead, Write};

fn main() {
    let mut profile = String::new();
    let mut scratch: Option<String> = None;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" => {
                i += 1;
                profile = args.get(i).cloned().unwrap_or_default();
            }
            "--scratch" => {
                i += 1;
                scratch = args.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }
    // In a real worker the sandbox profile is applied here (before serving).
    let _ = scratch;
    println!("ready {profile}");
    let stdout = std::io::stdout();
    let stdin = std::io::stdin();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line == "bye" {
            break;
        }
        let _ = writeln!(out, "ack:{line}");
        let _ = out.flush();
    }
}
