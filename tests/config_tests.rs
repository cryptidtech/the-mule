use the_mule::config::{assign_peers, TestConfig};

fn parse_config(yaml: &str) -> TestConfig {
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn parse_real_smoke_test_yaml() {
    let yaml = std::fs::read_to_string("examples/smoke-test-5peer.yaml")
        .expect("smoke-test-5peer.yaml should exist");
    let config: TestConfig = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(config.test_name, "smoke-test-5peer");
    assert_eq!(config.peers.len(), 5);
    assert_eq!(config.commands.len(), 16);
    assert_eq!(config.redis.port, 6399);
    assert_eq!(config.base_port, 11984);

    let assignments = assign_peers(&config);
    assert_eq!(assignments.len(), 5);
}

#[test]
fn missing_required_field_test_name() {
    let yaml = r#"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
        docker_image: "test:latest"
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
        test_name: "test"
        redis:
          port: "not-a-number"
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
        docker_image: "test:latest"
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
        test_name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
        docker_image: "test:latest"
        base_port: 10000
        peers: []
        commands: []
        "#,
    );
    assert!(config.peers.is_empty());
}

#[test]
fn peer_env_is_optional() {
    let config = parse_config(
        r#"
        test_name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
        docker_image: "test:latest"
        base_port: 10000
        peers:
          - name: alice
          - name: bob
            env:
              RUST_LOG: debug
        commands: []
        "#,
    );
    assert!(config.peers[0].env.is_none());
    assert!(config.peers[1].env.is_some());
}

#[test]
fn default_timeout_values() {
    let config = parse_config(
        r#"
        test_name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
        docker_image: "test:latest"
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
        test_name: "test"
        redis:
          port: 6379
          image: "redis:7-alpine"
        hosts:
          - address: host0
            ssh_user: user
            ssh_auth: agent
        docker_image: "test:latest"
        base_port: 10000
        peers: []
        commands: []
        timeout:
          startup: 120
        "#,
    );
    assert_eq!(config.timeout.startup, 120);
    assert_eq!(config.timeout.shutdown, 30);
}
