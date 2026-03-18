use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(transparent)]
pub struct PeerName(String);

impl PeerName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(transparent)]
pub struct HostName(String);

impl HostName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.0)
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_filter_str(&self) -> &str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct TestConfig {
    pub name: String,
    #[serde(default)]
    pub timeout: TimeoutConfig,
    pub redis: RedisConfig,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub remove_images: bool,
    #[serde(default, deserialize_with = "deserialize_environment")]
    pub peer_environment: HashMap<String, String>,
    pub hosts: Vec<HostConfig>,
    pub peers: Vec<PeerConfig>,
    pub commands: Vec<TestCommand>,
    #[serde(default)]
    pub log_level: Option<LogLevel>,
}

#[derive(Deserialize, Clone)]
pub struct RedisConfig {
    pub port: u16,
    pub image: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HostConfig {
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    pub ssh_user: String,
    pub ssh_auth: String,
    pub base_port: u16,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl HostConfig {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.address)
    }
    pub fn host_name(&self) -> HostName {
        HostName::new(self.display_name())
    }
}

#[derive(Deserialize, Clone)]
pub struct PeerConfig {
    pub name: PeerName,
    pub image: String,
    #[serde(default)]
    pub bootstrap: Vec<PeerName>,
    #[serde(default, deserialize_with = "deserialize_environment")]
    pub environment: HashMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_string_or_list")]
    pub runs_on: Vec<String>,
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

/// Deserialize `runs_on` from either a single string or a list of strings.
fn deserialize_string_or_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StringOrListVisitor;

    impl<'de> Visitor<'de> for StringOrListVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut result = Vec::new();
            while let Some(entry) = seq.next_element::<String>()? {
                result.push(entry);
            }
            Ok(result)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(StringOrListVisitor)
}

#[derive(Deserialize, Clone)]
pub struct TestCommand {
    pub time: u64,
    pub peer: PeerName,
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
#[derive(Clone, Debug)]
pub struct PeerAssignment {
    pub peer_name: PeerName,
    pub host: HostConfig,
    pub port: u16,
    pub listen_addr: String,
    pub extra_env: HashMap<String, String>,
    pub docker_image: String,
}

/// Assign peers to hosts round-robin with unique ports per host.
///
/// Peers with `runs_on` tags are matched to hosts whose `tags` contain all
/// required tags. Peers with no `runs_on` tags match all hosts.
/// Returns an error if any peer's tags cannot be satisfied by any host.
pub fn assign_peers(config: &TestConfig) -> Result<Vec<PeerAssignment>, String> {
    let mut sorted_peers: Vec<&PeerConfig> = config.peers.iter().collect();
    sorted_peers.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));

    // Group peers by their sorted runs_on tags
    let mut groups: BTreeMap<Vec<String>, Vec<&PeerConfig>> = BTreeMap::new();
    for peer in &sorted_peers {
        let mut key = peer.runs_on.clone();
        key.sort();
        groups.entry(key).or_default().push(peer);
    }

    // Track next available port per host index (shared across groups)
    let mut port_counters: HashMap<usize, u16> = HashMap::new();
    let mut assignments = Vec::new();

    for (required_tags, peers) in &groups {
        // Find matching host indices
        let matching_indices: Vec<usize> = if required_tags.is_empty() {
            (0..config.hosts.len()).collect()
        } else {
            config
                .hosts
                .iter()
                .enumerate()
                .filter(|(_, h)| required_tags.iter().all(|t| h.tags.contains(t)))
                .map(|(i, _)| i)
                .collect()
        };

        if matching_indices.is_empty() {
            let peer_names: Vec<&str> = peers.iter().map(|p| p.name.as_str()).collect();
            return Err(format!(
                "no host matches tags {:?} required by peer(s): {}",
                required_tags,
                peer_names.join(", ")
            ));
        }

        // Round-robin within matching hosts
        for (i, peer) in peers.iter().enumerate() {
            let host_idx = matching_indices[i % matching_indices.len()];
            let host = &config.hosts[host_idx];
            let port_offset = port_counters.entry(host_idx).or_insert(0);
            let port = host.base_port + *port_offset;
            *port_offset += 1;

            // Merge environments: global defaults, then peer-specific overrides
            let mut extra_env = config.peer_environment.clone();
            for (k, v) in &peer.environment {
                extra_env.insert(k.clone(), v.clone());
            }

            assignments.push(PeerAssignment {
                peer_name: peer.name.clone(),
                host: host.clone(),
                port,
                listen_addr: format!("/ip4/0.0.0.0/udp/{port}/quic-v1"),
                extra_env,
                docker_image: peer.image.clone(),
            });
        }
    }

    // Sort final assignments by peer_name for deterministic output
    assignments.sort_by(|a, b| a.peer_name.cmp(&b.peer_name));
    Ok(assignments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> TestConfig {
        serde_yaml::from_str(
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
            commands: []
            "#,
        )
        .unwrap()
    }

    #[test]
    fn parse_minimal_config() {
        let config = minimal_config();
        assert_eq!(config.name, "test");
        assert_eq!(config.peers.len(), 2);
        assert_eq!(config.timeout.startup, 60);
        assert_eq!(config.timeout.shutdown, 30);
    }

    #[test]
    fn parse_full_config_with_timeout() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            name: "full"
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
        let assignments = assign_peers(&config).unwrap();
        assert_eq!(assignments.len(), 2);
        // Alphabetical: alice, bob — both go to host0
        assert_eq!(assignments[0].peer_name.as_str(), "alice");
        assert_eq!(assignments[0].port, 10000);
        assert_eq!(assignments[0].docker_image, "test:latest");
        assert_eq!(assignments[1].peer_name.as_str(), "bob");
        assert_eq!(assignments[1].port, 10001);
        assert_eq!(assignments[1].docker_image, "test:latest");
    }

    #[test]
    fn round_robin_assignment_two_hosts() {
        let config: TestConfig = serde_yaml::from_str(
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
        let assignments = assign_peers(&config).unwrap();
        // Alphabetical: alice(host0:10000), bob(host1:10000), charlie(host0:10001)
        assert_eq!(assignments[0].peer_name.as_str(), "alice");
        assert_eq!(assignments[0].host.address, "host0");
        assert_eq!(assignments[0].port, 10000);
        assert_eq!(assignments[0].docker_image, "test:latest");
        assert_eq!(assignments[1].peer_name.as_str(), "bob");
        assert_eq!(assignments[1].host.address, "host1");
        assert_eq!(assignments[1].port, 10000);
        assert_eq!(assignments[2].peer_name.as_str(), "charlie");
        assert_eq!(assignments[2].host.address, "host0");
        assert_eq!(assignments[2].port, 10001);
    }

    #[test]
    fn round_robin_two_hosts_different_base_ports() {
        let config: TestConfig = serde_yaml::from_str(
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
        let assignments = assign_peers(&config).unwrap();
        // alice->host0:10000, bob->host1:20000, charlie->host0:10001, dave->host1:20001
        assert_eq!(assignments[0].peer_name.as_str(), "alice");
        assert_eq!(assignments[0].port, 10000);
        assert_eq!(assignments[0].docker_image, "img-a:latest");
        assert_eq!(assignments[1].peer_name.as_str(), "bob");
        assert_eq!(assignments[1].port, 20000);
        assert_eq!(assignments[1].docker_image, "img-b:latest");
        assert_eq!(assignments[2].peer_name.as_str(), "charlie");
        assert_eq!(assignments[2].port, 10001);
        assert_eq!(assignments[3].peer_name.as_str(), "dave");
        assert_eq!(assignments[3].port, 20001);
    }

    #[test]
    fn per_peer_image() {
        let config: TestConfig = serde_yaml::from_str(
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
        )
        .unwrap();
        let assignments = assign_peers(&config).unwrap();
        assert_eq!(assignments[0].docker_image, "rust-peer:latest");
        assert_eq!(assignments[1].docker_image, "python-peer:latest");
    }

    #[test]
    fn host_display_name_defaults_to_address() {
        let config = minimal_config();
        assert_eq!(config.hosts[0].display_name(), "host0");
    }

    #[test]
    fn host_display_name_uses_name_field() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            name: "test"
            redis:
              port: 6379
              image: "redis:7-alpine"
            hosts:
              - address: 192.168.1.10
                name: "web-server"
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
            peers:
              - name: alice
                image: "test:latest"
            commands: []
            "#,
        )
        .unwrap();
        assert_eq!(config.hosts[0].display_name(), "web-server");
    }

    #[test]
    fn peer_environment_merges_with_global() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            name: "test"
            redis:
              port: 6379
              image: "redis:7-alpine"
            peer_environment:
              RUST_LOG: debug
              GLOBAL_VAR: hello
            hosts:
              - address: host0
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
            peers:
              - name: alice
                image: "test:latest"
                environment:
                  RUST_LOG: info
              - name: bob
                image: "test:latest"
            commands: []
            "#,
        )
        .unwrap();
        let assignments = assign_peers(&config).unwrap();
        // alice overrides RUST_LOG but inherits GLOBAL_VAR
        assert_eq!(assignments[0].extra_env.get("RUST_LOG").unwrap(), "info");
        assert_eq!(assignments[0].extra_env.get("GLOBAL_VAR").unwrap(), "hello");
        // bob inherits both from global
        assert_eq!(assignments[1].extra_env.get("RUST_LOG").unwrap(), "debug");
        assert_eq!(assignments[1].extra_env.get("GLOBAL_VAR").unwrap(), "hello");
    }

    #[test]
    fn runs_on_single_string() {
        let config: TestConfig = serde_yaml::from_str(
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
                tags: [gpu]
            peers:
              - name: alice
                image: "test:latest"
                runs_on: gpu
            commands: []
            "#,
        )
        .unwrap();
        assert_eq!(config.peers[0].runs_on, vec!["gpu"]);
        let assignments = assign_peers(&config).unwrap();
        assert_eq!(assignments[0].host.address, "host0");
    }

    #[test]
    fn runs_on_tag_matching() {
        let config: TestConfig = serde_yaml::from_str(
            r#"
            name: "test"
            redis:
              port: 6379
              image: "redis:7-alpine"
            hosts:
              - address: gpu-host
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
                tags: [gpu, fast]
              - address: cpu-host
                ssh_user: user
                ssh_auth: agent
                base_port: 10000
                tags: [cpu]
            peers:
              - name: alice
                image: "test:latest"
                runs_on: [gpu]
              - name: bob
                image: "test:latest"
                runs_on: [cpu]
              - name: charlie
                image: "test:latest"
            commands: []
            "#,
        )
        .unwrap();
        let assignments = assign_peers(&config).unwrap();
        // alice -> gpu-host, bob -> cpu-host, charlie -> any (round-robin across both)
        let alice = assignments.iter().find(|a| a.peer_name.as_str() == "alice").unwrap();
        assert_eq!(alice.host.address, "gpu-host");
        let bob = assignments.iter().find(|a| a.peer_name.as_str() == "bob").unwrap();
        assert_eq!(bob.host.address, "cpu-host");
    }

    #[test]
    fn runs_on_no_matching_host_errors() {
        let config: TestConfig = serde_yaml::from_str(
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
                tags: [cpu]
            peers:
              - name: alice
                image: "test:latest"
                runs_on: [gpu]
            commands: []
            "#,
        )
        .unwrap();
        let result = assign_peers(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("gpu"));
    }

    #[test]
    fn remove_images_defaults_to_false() {
        let config = minimal_config();
        assert!(!config.remove_images);
    }

    #[test]
    fn peer_environment_defaults_to_empty() {
        let config = minimal_config();
        assert!(config.peer_environment.is_empty());
    }

    #[test]
    fn tags_defaults_to_empty() {
        let config = minimal_config();
        assert!(config.hosts[0].tags.is_empty());
    }

    #[test]
    fn runs_on_defaults_to_empty() {
        let config = minimal_config();
        assert!(config.peers[0].runs_on.is_empty());
    }
}
