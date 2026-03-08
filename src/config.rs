use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

#[derive(Deserialize, Clone)]
pub struct TestConfig {
    pub test_name: String,
    pub redis: RedisConfig,
    #[serde(default)]
    pub images: Vec<String>,
    pub hosts: Vec<HostConfig>,
    pub peers: Vec<PeerConfig>,
    pub commands: Vec<TestCommand>,
    #[serde(default)]
    pub timeout: TimeoutConfig,
}

#[derive(Deserialize, Clone)]
pub struct RedisConfig {
    pub port: u16,
    pub image: String,
}

#[derive(Deserialize, Clone)]
pub struct HostConfig {
    pub address: String,
    pub ssh_user: String,
    pub ssh_auth: String,
    pub base_port: u16,
}

#[derive(Deserialize, Clone)]
pub struct PeerConfig {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub bootstrap: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_environment")]
    pub environment: HashMap<String, String>,
}

/// Deserialize `environment` from either map syntax (`KEY: VALUE`) or list syntax (`- KEY=VALUE`).
fn deserialize_environment<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct EnvironmentVisitor;

    impl<'de> Visitor<'de> for EnvironmentVisitor {
        type Value = HashMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of KEY: VALUE or a list of KEY=VALUE strings")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut result = HashMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                result.insert(key, value);
            }
            Ok(result)
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut result = HashMap::new();
            while let Some(entry) = seq.next_element::<String>()? {
                let (key, value) = entry.split_once('=').ok_or_else(|| {
                    de::Error::custom(format!(
                        "environment list entry must be KEY=VALUE, got: {entry}"
                    ))
                })?;
                result.insert(key.to_owned(), value.to_owned());
            }
            Ok(result)
        }
    }

    deserializer.deserialize_any(EnvironmentVisitor)
}

#[derive(Deserialize, Clone)]
pub struct TestCommand {
    pub time: u64,
    pub peer: String,
    pub command: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct TimeoutConfig {
    #[serde(default = "default_startup_timeout")]
    pub startup: u64,
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown: u64,
}

fn default_startup_timeout() -> u64 {
    60
}
fn default_shutdown_timeout() -> u64 {
    30
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            startup: 60,
            shutdown: 30,
        }
    }
}

/// Computed at startup: maps each peer to a host + port.
#[derive(Clone)]
pub struct PeerAssignment {
    pub peer_name: String,
    pub host: HostConfig,
    pub port: u16,
    pub listen_addr: String,
    pub extra_env: HashMap<String, String>,
    pub docker_image: String,
}

/// Assign peers to hosts round-robin with unique ports per host.
pub fn assign_peers(config: &TestConfig) -> Vec<PeerAssignment> {
    let mut sorted_peers: Vec<&PeerConfig> = config.peers.iter().collect();
    sorted_peers.sort_by(|a, b| a.name.cmp(&b.name));

    // Track next available port per host index
    let mut port_counters: HashMap<usize, u16> = HashMap::new();
    let mut assignments = Vec::new();

    for (i, peer) in sorted_peers.iter().enumerate() {
        let host_idx = i % config.hosts.len();
        let host = &config.hosts[host_idx];
        let port_offset = port_counters.entry(host_idx).or_insert(0);
        let port = host.base_port + *port_offset;
        *port_offset += 1;

        let extra_env = peer.environment.clone();

        assignments.push(PeerAssignment {
            peer_name: peer.name.clone(),
            host: host.clone(),
            port,
            listen_addr: format!("/ip4/0.0.0.0/udp/{port}/quic-v1"),
            extra_env,
            docker_image: peer.image.clone(),
        });
    }

    assignments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> TestConfig {
        serde_yaml::from_str(
            r#"
            test_name: "test"
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
            commands: []
            "#,
        )
        .unwrap()
    }

    #[test]
    fn parse_minimal_config() {
        let config = minimal_config();
        assert_eq!(config.test_name, "test");
        assert_eq!(config.peers.len(), 2);
        assert_eq!(config.timeout.startup, 60);
        assert_eq!(config.timeout.shutdown, 30);
    }

    #[test]
    fn parse_full_config_with_timeout() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            test_name: "full"
            redis:
              port: 6399
              image: "redis:7-alpine"
            hosts:
              - address: host0
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
            peers:
              - name: alice
                image: "test:latest"
            commands: []
            timeout:
              startup: 120
              shutdown: 45
            "#,
        )
        .unwrap();
        assert_eq!(config.timeout.startup, 120);
        assert_eq!(config.timeout.shutdown, 45);
    }

    #[test]
    fn parse_config_with_images() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            test_name: "test"
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
        )
        .unwrap();
        assert_eq!(config.images.len(), 2);
        assert_eq!(config.images[0], "alpine:latest");
    }

    #[test]
    fn images_defaults_to_empty() {
        let config = minimal_config();
        assert!(config.images.is_empty());
    }

    #[test]
    fn round_robin_assignment_single_host() {
        let config = minimal_config();
        let assignments = assign_peers(&config);
        assert_eq!(assignments.len(), 2);
        // Alphabetical: alice, bob — both go to host0
        assert_eq!(assignments[0].peer_name, "alice");
        assert_eq!(assignments[0].port, 10000);
        assert_eq!(assignments[0].docker_image, "test:latest");
        assert_eq!(assignments[1].peer_name, "bob");
        assert_eq!(assignments[1].port, 10001);
        assert_eq!(assignments[1].docker_image, "test:latest");
    }

    #[test]
    fn round_robin_assignment_two_hosts() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            test_name: "test"
            redis:
              port: 6379
              image: "redis:7-alpine"
            hosts:
              - address: host0
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
              - address: host1
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
            peers:
              - name: charlie
                image: "test:latest"
              - name: alice
                image: "test:latest"
              - name: bob
                image: "test:latest"
            commands: []
            "#,
        )
        .unwrap();
        let assignments = assign_peers(&config);
        // Alphabetical: alice(host0:10000), bob(host1:10000), charlie(host0:10001)
        assert_eq!(assignments[0].peer_name, "alice");
        assert_eq!(assignments[0].host.address, "host0");
        assert_eq!(assignments[0].port, 10000);
        assert_eq!(assignments[0].docker_image, "test:latest");
        assert_eq!(assignments[1].peer_name, "bob");
        assert_eq!(assignments[1].host.address, "host1");
        assert_eq!(assignments[1].port, 10000);
        assert_eq!(assignments[2].peer_name, "charlie");
        assert_eq!(assignments[2].host.address, "host0");
        assert_eq!(assignments[2].port, 10001);
    }

    #[test]
    fn round_robin_two_hosts_different_base_ports() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            test_name: "test"
            redis:
              port: 6379
              image: "redis:7-alpine"
            hosts:
              - address: host0
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
              - address: host1
                ssh_user: user
                ssh_auth: agent
                base_port: 20000
            peers:
              - name: alice
                image: "img-a:latest"
              - name: bob
                image: "img-b:latest"
              - name: charlie
                image: "img-a:latest"
              - name: dave
                image: "img-b:latest"
            commands: []
            "#,
        )
        .unwrap();
        let assignments = assign_peers(&config);
        // alice->host0:10000, bob->host1:20000, charlie->host0:10001, dave->host1:20001
        assert_eq!(assignments[0].peer_name, "alice");
        assert_eq!(assignments[0].port, 10000);
        assert_eq!(assignments[0].docker_image, "img-a:latest");
        assert_eq!(assignments[1].peer_name, "bob");
        assert_eq!(assignments[1].port, 20000);
        assert_eq!(assignments[1].docker_image, "img-b:latest");
        assert_eq!(assignments[2].peer_name, "charlie");
        assert_eq!(assignments[2].port, 10001);
        assert_eq!(assignments[3].peer_name, "dave");
        assert_eq!(assignments[3].port, 20001);
    }

    #[test]
    fn per_peer_image() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            test_name: "test"
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
        )
        .unwrap();
        let assignments = assign_peers(&config);
        assert_eq!(assignments[0].docker_image, "rust-peer:latest");
        assert_eq!(assignments[1].docker_image, "python-peer:latest");
    }
}
