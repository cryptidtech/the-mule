use the_mule::config::{assign_peers, TestConfig};

fn parse_config(yaml: &str) -> TestConfig {
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn parse_real_smoke_test_yaml() {
    let yaml = std::fs::read_to_string("examples/smoke-test-5peer.yaml")
        .expect("smoke-test-5peer.yaml should exist");
    let config: TestConfig = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(config.name, "smoke-test-5peer");
    assert_eq!(config.peers.len(), 5);
    assert_eq!(config.commands.len(), 11);
    assert_eq!(config.redis.port, 6399);
    assert_eq!(config.images.len(), 3);

    let assignments = assign_peers(&config).unwrap();
    assert_eq!(assignments.len(), 5);
    // Verify per-peer images are propagated
    for a in &assignments {
        assert!(!a.docker_image.is_empty());
    }
}

#[test]
fn missing_required_field_name() {
    let yaml = r#"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers: []
        commands: []
    "#;
    let result: Result<TestConfig, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err());
}

#[test]
fn invalid_type_port_string() {
    let yaml = r#"
        name: "test"
        redis:
          port: "not-a-number"
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers: []
        commands: []
    "#;
    let result: Result<TestConfig, _> = serde_yaml::from_str(yaml);
    assert!(result.is_err());
}

#[test]
fn empty_peers_is_valid() {
    let config = parse_config(
        r#"
        name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers: []
        commands: []
        "#,
    );
    assert!(config.peers.is_empty());
}

#[test]
fn peer_environment_is_optional() {
    let config = parse_config(
        r#"
        name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers:
          - name: alice
            image: "test:latest"
          - name: bob
            image: "test:latest"
            environment:
              RUST_LOG: debug
        commands: []
        "#,
    );
    assert!(config.peers[0].environment.is_empty());
    assert_eq!(config.peers[1].environment.get("RUST_LOG").unwrap(), "debug");
}

#[test]
fn peer_environment_list_syntax() {
    let config = parse_config(
        r#"
        name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers:
          - name: alice
            image: "test:latest"
            environment:
              - RUST_LOG=info
              - MY_VAR=hello=world
        commands: []
        "#,
    );
    assert_eq!(config.peers[0].environment.get("RUST_LOG").unwrap(), "info");
    assert_eq!(config.peers[0].environment.get("MY_VAR").unwrap(), "hello=world");
}

#[test]
fn default_timeout_values() {
    let config = parse_config(
        r#"
        name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers: []
        commands: []
        "#,
    );
    assert_eq!(config.timeout.startup, 60);
    assert_eq!(config.timeout.shutdown, 30);
}

#[test]
fn partial_timeout_uses_defaults() {
    let config = parse_config(
        r#"
        name: "test"
        timeout:
          startup: 120
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers: []
        commands: []
        "#,
    );
    assert_eq!(config.timeout.startup, 120);
    assert_eq!(config.timeout.shutdown, 30);
}

#[test]
fn images_field_deserialization() {
    let config = parse_config(
        r#"
        name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        images:
          - "alpine:latest"
          - "nginx:latest"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers:
          - name: alice
            image: "alpine:latest"
        commands: []
        "#,
    );
    assert_eq!(config.images.len(), 2);
    assert_eq!(config.images[0], "alpine:latest");
    assert_eq!(config.images[1], "nginx:latest");
}

#[test]
fn per_peer_docker_image_in_assignment() {
    let config = parse_config(
        r#"
        name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
            base_port: 10000
        peers:
          - name: alice
            image: "rust-peer:latest"
          - name: bob
            image: "python-peer:latest"
        commands: []
        "#,
    );
    let assignments = assign_peers(&config).unwrap();
    assert_eq!(assignments[0].peer_name.as_str(), "alice");
    assert_eq!(assignments[0].docker_image, "rust-peer:latest");
    assert_eq!(assignments[1].peer_name.as_str(), "bob");
    assert_eq!(assignments[1].docker_image, "python-peer:latest");
}
