use std::process::Command;

fn require_docker() -> bool {
    Command::new("docker")
        .args(["info"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ensure_alpine() {
    let out = Command::new("docker")
        .args(["image", "inspect", "alpine:latest"])
        .output()
        .unwrap();
    if !out.status.success() {
        let pull = Command::new("docker")
            .args(["pull", "alpine:latest"])
            .output()
            .unwrap();
        assert!(pull.status.success(), "failed to pull alpine:latest");
    }
}

fn cleanup_container(name: &str) {
    let _ = Command::new("docker").args(["rm", "-f", name]).output();
}

#[test]
fn docker_inspect_image() {
    if !require_docker() {
        eprintln!("skipping: Docker daemon not available");
        return;
    }
    ensure_alpine();
    let out = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", "alpine:latest"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout);
    assert!(
        id.trim().starts_with("sha256:"),
        "expected sha256 prefix, got: {}",
        id.trim()
    );
}

#[test]
fn docker_inspect_missing_image() {
    if !require_docker() {
        eprintln!("skipping: Docker daemon not available");
        return;
    }
    let out = Command::new("docker")
        .args(["image", "inspect", "nonexistent-image-xyz:latest"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn docker_run_and_stop_container() {
    if !require_docker() {
        eprintln!("skipping: Docker daemon not available");
        return;
    }
    ensure_alpine();
    let name = "tm-test-run-stop";
    cleanup_container(name);

    // Start container with env var
    let run = Command::new("docker")
        .args([
            "run", "-d", "--name", name, "-e", "TEST_VAR=hello", "alpine:latest", "sleep", "60",
        ])
        .output()
        .unwrap();
    assert!(run.status.success(), "failed to start container");

    // Verify running
    let inspect = Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", name])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&inspect.stdout).trim(),
        "true"
    );

    // Verify env var
    let env = Command::new("docker")
        .args(["exec", name, "printenv", "TEST_VAR"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&env.stdout).trim(), "hello");

    // Remove and verify gone
    let rm = Command::new("docker")
        .args(["rm", "-f", name])
        .output()
        .unwrap();
    assert!(rm.status.success());
    let gone = Command::new("docker")
        .args(["inspect", name])
        .output()
        .unwrap();
    assert!(!gone.status.success());
}

#[test]
fn docker_run_with_port_mapping() {
    if !require_docker() {
        eprintln!("skipping: Docker daemon not available");
        return;
    }
    ensure_alpine();
    let name = "tm-test-port-map";
    cleanup_container(name);

    let run = Command::new("docker")
        .args([
            "run", "-d", "--name", name, "-p", "19999:19999/udp", "alpine:latest", "sleep", "30",
        ])
        .output()
        .unwrap();
    assert!(run.status.success(), "failed to start container");

    let port = Command::new("docker")
        .args(["port", name])
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&port.stdout);
    assert!(
        info.contains("19999/udp"),
        "expected port mapping in: {info}"
    );

    cleanup_container(name);
}

#[test]
fn docker_save_and_load_image() {
    if !require_docker() {
        eprintln!("skipping: Docker daemon not available");
        return;
    }
    ensure_alpine();
    let archive = "/tmp/tm-test-docker-save.tar.gz";

    let save = Command::new("sh")
        .args([
            "-c",
            &format!("docker save alpine:latest | gzip > {archive}"),
        ])
        .output()
        .unwrap();
    assert!(save.status.success(), "docker save failed");
    assert!(
        std::fs::metadata(archive).unwrap().len() > 0,
        "archive should be non-empty"
    );

    let load = Command::new("docker")
        .args(["load", "-i", archive])
        .output()
        .unwrap();
    assert!(load.status.success(), "docker load failed");

    let _ = std::fs::remove_file(archive);
}
