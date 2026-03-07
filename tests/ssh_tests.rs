use the_mule::ssh_mgr;

#[test]
fn shellexpand_tilde_to_home() {
    let expanded = ssh_mgr::shellexpand("~/.ssh/id_ed25519");
    let home = std::env::var("HOME").unwrap();
    assert_eq!(
        expanded,
        std::path::PathBuf::from(format!("{home}/.ssh/id_ed25519"))
    );
}

#[test]
fn load_ssh_key_from_disk() {
    // Try common SSH key paths — skip if none found
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => {
            eprintln!("skipping: HOME not set");
            return;
        }
    };

    let candidates = [
        format!("{home}/.ssh/id_ed25519"),
        format!("{home}/.ssh/id_rsa"),
    ];

    let key_path = candidates.iter().find(|p| std::path::Path::new(p).exists());
    match key_path {
        Some(path) => {
            let contents = std::fs::read_to_string(path).unwrap();
            assert!(!contents.is_empty(), "key file should not be empty");
            // Basic sanity: should start with a PEM-like header or OpenSSH header
            assert!(
                contents.starts_with("-----BEGIN") || contents.starts_with("ssh-"),
                "key file should start with a known header"
            );
        }
        None => {
            eprintln!("skipping: no SSH key found at ~/.ssh/id_ed25519 or ~/.ssh/id_rsa");
        }
    }
}

#[test]
fn ssh_agent_list_identities() {
    // Skip if SSH_AUTH_SOCK is not set
    let sock = match std::env::var("SSH_AUTH_SOCK") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            eprintln!("skipping: SSH_AUTH_SOCK not set");
            return;
        }
    };

    // Verify the socket path exists — skip if stale
    if !std::path::Path::new(&sock).exists() {
        eprintln!("skipping: SSH_AUTH_SOCK path does not exist: {sock}");
        return;
    }

    // Use ssh2 directly to test agent connectivity
    let session = ssh2::Session::new().expect("create session");
    let mut agent = session.agent().expect("init agent");
    agent.connect().expect("connect to agent");
    agent.list_identities().expect("list identities");
    let identities = agent.identities().expect("get identities");
    // Just verify we can enumerate — may be empty in CI
    eprintln!("SSH agent has {} identities", identities.len());
}
