//! P63 — defense-in-depth proof for the non-secret backend launch path.
//!
//! The compatibility planner has no secret input and can emit only reviewed
//! non-secret names. `ProcessTransport` independently enforces the child
//! environment boundary, so registry/install metadata cannot widen it by
//! passing an arbitrary string pair. No real credentials or network are used.

use agentcowork_acp::{
    AcpTransport, ProcessTransport, ProcessTransportError, ProviderBinding, backend_spec, plan_env,
};

fn non_secret_binding<'a>(
    provider: &'a str,
    model: &'a str,
    key_env: &'a str,
    base_url: Option<&'a str>,
    base_url_env: Option<&'a str>,
) -> ProviderBinding<'a> {
    ProviderBinding {
        provider,
        model,
        key_env,
        base_url,
        base_url_env,
    }
}

#[test]
fn legacy_planner_has_no_key_pair_and_accepts_a_safe_binding() {
    let spec = backend_spec("claude");
    let binding = non_secret_binding(
        "anthropic",
        "claude-sonnet-4",
        "ANTHROPIC_API_KEY",
        Some("https://api.anthropic.com/v1"),
        None,
    );
    let pairs = plan_env(&spec, &binding).expect("safe non-secret binding");
    assert_eq!(
        pairs,
        vec![
            (
                "ANTHROPIC_BASE_URL".to_string(),
                "https://api.anthropic.com/v1".to_string()
            ),
            ("ANTHROPIC_MODEL".to_string(), "claude-sonnet-4".to_string()),
        ]
    );
    assert!(pairs.iter().all(|(name, _)| name != "ANTHROPIC_API_KEY"));
}

#[test]
fn malicious_metadata_names_are_refused_before_spawn() {
    for name in [
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "BASH_ENV",
        "NODE_OPTIONS",
        "NODE_PATH",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "DATABASE_URL",
        "KUBECONFIG",
        "EVERYAIOS_VAULT_PASSPHRASE",
        "AGENTCOWORK_VAULT_PASSPHRASE",
        "ANTHROPIC_API_KEY",
        "UNKNOWN_AGENT_FLAG",
    ] {
        let error = match ProcessTransport::spawn(
            "sh",
            &["-c", "exit 0"],
            &[(name, "attacker-controlled")],
        ) {
            Err(error) => error,
            Ok(_) => panic!("unreviewed child metadata must be refused: {name}"),
        };
        assert!(
            matches!(error, ProcessTransportError::DisallowedEnvVar { name: ref rejected_name, .. } if rejected_name == name),
            "unexpected error for {name}: {error}"
        );
    }
}

#[test]
fn malicious_values_for_reviewed_names_are_refused() {
    for (name, value) in [
        ("ANTHROPIC_MODEL", "model\nINJECTED=1"),
        ("ANTHROPIC_MODEL", "https://attacker.invalid/"),
        ("ANTHROPIC_MODEL", "sk-live-value"),
        ("ANTHROPIC_MODEL", "model\0INJECTED"),
        (
            "ANTHROPIC_BASE_URL",
            "https://user:password@example.invalid/v1",
        ),
        (
            "ANTHROPIC_BASE_URL",
            "https://example.invalid/v1?token=exfiltrate",
        ),
        (
            "ANTHROPIC_BASE_URL",
            "https://example.invalid/v1#token=exfiltrate",
        ),
        ("ANTHROPIC_BASE_URL", "file:///etc/passwd"),
        (
            "ANTHROPIC_BASE_URL",
            "http://169.254.169.254/latest/meta-data/",
        ),
        ("ANTHROPIC_BASE_URL", "http://192.168.1.10/v1"),
        ("ANTHROPIC_BASE_URL", "http://2130706433/v1"),
    ] {
        let error = match ProcessTransport::spawn("sh", &["-c", "exit 0"], &[(name, value)]) {
            Err(error) => error,
            Ok(_) => panic!("unsafe reviewed value must be refused: {name}"),
        };
        assert!(
            matches!(
                error,
                ProcessTransportError::InvalidEnvValue { .. }
                    | ProcessTransportError::DisallowedEnvVar { .. }
            ),
            "unexpected error for {name}: {error}"
        );
        assert!(!error.to_string().contains("password"));
        assert!(!error.to_string().contains("exfiltrate"));
    }
}

#[cfg(unix)]
fn selected_child_values(env: &[(&str, &str)]) -> String {
    // The probe writes only the three selected observations, never the full
    // environment. Values are fixed test literals, and the temporary path is
    // made from a numeric process id.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("p63-env-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let out = dir.join("selected-values.txt");
    let script = format!(
        "printf 'base=%s\\nmodel=%s\\nkey_present=%s\\n' \\\n         \"${{ANTHROPIC_BASE_URL-}}\" \\\n         \"${{ANTHROPIC_MODEL-}}\" \\\n         \"${{ANTHROPIC_API_KEY+x}}\" > {}",
        out.display()
    );
    let mut transport = ProcessTransport::spawn("sh", &["-c", script.as_str()], env)
        .expect("spawn the probe child");
    let mut content = String::new();
    for _ in 0..200 {
        if let Ok(value) = std::fs::read_to_string(&out) {
            if !value.is_empty() {
                content = value;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    transport.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
    content
}

#[cfg(unix)]
#[test]
fn reviewed_non_secret_values_reach_a_real_child_without_a_key() {
    let env = [
        ("ANTHROPIC_BASE_URL", "http://127.0.0.1:9/v1"),
        ("ANTHROPIC_MODEL", "claude-sonnet-4"),
    ];
    let selected = selected_child_values(&env);
    assert!(selected.contains("base=http://127.0.0.1:9/v1"));
    assert!(selected.contains("model=claude-sonnet-4"));
    assert!(selected.contains("key_present=\n"));
    assert!(!selected.contains("ANTHROPIC_API_KEY="));
}

#[test]
fn a_config_file_agent_injects_nothing_and_is_refused_with_its_path() {
    let spec = backend_spec("codex");
    let binding = non_secret_binding(
        "openai",
        "gpt-5",
        "OPENAI_API_KEY",
        Some("https://api.openai.com/v1"),
        None,
    );
    match plan_env(&spec, &binding) {
        Err(agentcowork_acp::BackendError::ConfigFileOnly { file, .. }) => {
            assert!(file.contains("config.toml"), "got {file}");
        }
        other => panic!("a config-file agent must refuse, got {other:?}"),
    }
}
