//! Docker CLI helpers: every call is a `tokio::process` with a timeout and a clean
//! environment. Nothing here builds or starts anything: builds and starts run in a
//! visible terminal (`engine`).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::util::proc::Output;

/// The docker executable: `[devcontainer] docker` or `docker` on PATH.
pub fn docker_bin(configured: &str) -> String {
    let c = configured.trim();
    if c.is_empty() || c == "docker" {
        crate::util::which_path("docker").map(|p| p.display().to_string()).unwrap_or_else(|| "docker".into())
    } else {
        crate::config::expand_tilde(c).display().to_string()
    }
}

pub async fn run(docker: &str, args: &[&str], timeout: Duration) -> Result<Output, String> {
    let mut cmd = tokio::process::Command::new(docker);
    cmd.args(args).current_dir("/");
    crate::util::proc::run_cmd(cmd, timeout).await.map_err(|e| e.message)
}

/// `docker …` with `stdin` fed from memory (secrets and file contents never go in argv).
pub async fn run_stdin(docker: &str, args: &[&str], stdin: &[u8], timeout: Duration) -> Result<Output, String> {
    let mut cmd = tokio::process::Command::new(docker);
    cmd.args(args).current_dir("/");
    crate::util::proc::clean_env(&mut cmd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("cannot run {docker}: {e}"))?;
    let data = stdin.to_vec();
    let mut input = child.stdin.take();
    let feed = async move {
        if let Some(i) = input.as_mut() {
            let _ = i.write_all(&data).await;
            let _ = i.shutdown().await;
        }
        drop(input);
    };
    let work = async {
        feed.await;
        child.wait_with_output().await
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(out)) => Ok(Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(format!("docker failed: {e}")),
        Err(_) => Err(format!("docker timed out after {}s", timeout.as_secs())),
    }
}

/// The server version, `None` when docker is missing or the daemon is unreachable.
pub async fn server_version(docker: &str) -> Result<String, String> {
    let out = run(docker, &["version", "--format", "{{.Server.Version}}"], Duration::from_secs(8)).await?;
    if out.ok() && !out.stdout.trim().is_empty() {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(first_line(&out.message()))
    }
}

pub async fn compose_version(docker: &str) -> Option<String> {
    let out = run(docker, &["compose", "version", "--short"], Duration::from_secs(8)).await.ok()?;
    out.ok().then(|| out.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn first_line(s: &str) -> String {
    let l = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    crate::apps::detect::text::ellipsize(l, 300)
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PortMap {
    pub port: u16,
    pub proto: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MountInfo {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    pub destination: String,
}

/// What Workbench needs from `docker inspect`.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    /// `running`, `exited`, `created`, `paused`, `restarting`, `dead`.
    pub status: String,
    pub running: bool,
    pub image: String,
    pub created: String,
    pub ports: Vec<PortMap>,
    /// Address on its first network with one (Linux bridge: reachable from the host).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// That network's gateway: the host as the container sees it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    pub network_mode: String,
    #[serde(skip)]
    pub labels: BTreeMap<String, String>,
    #[serde(skip)]
    pub env: BTreeMap<String, String>,
    #[serde(skip)]
    pub mounts: Vec<MountInfo>,
    /// `Config.User`.
    #[serde(skip)]
    pub user: String,
}

impl ContainerInfo {
    pub fn label(&self, k: &str) -> Option<&str> {
        self.labels.get(k).map(String::as_str)
    }
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(12)]
    }
}

pub fn parse_inspect(v: &Value) -> Option<ContainerInfo> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
    let id = s("/Id");
    if id.is_empty() {
        return None;
    }
    let labels: BTreeMap<String, String> = v
        .pointer("/Config/Labels")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, x)| x.as_str().map(|x| (k.clone(), x.to_string()))).collect())
        .unwrap_or_default();
    let env: BTreeMap<String, String> = v
        .pointer("/Config/Env")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|e| e.as_str()?.split_once('=').map(|(k, x)| (k.to_string(), x.to_string()))).collect())
        .unwrap_or_default();
    let mut ports = vec![];
    if let Some(o) = v.pointer("/NetworkSettings/Ports").and_then(Value::as_object) {
        for (k, binds) in o {
            let (port, proto) = k.split_once('/').unwrap_or((k, "tcp"));
            let Ok(port) = port.parse::<u16>() else { continue };
            let list = binds.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                ports.push(PortMap { port, proto: proto.into(), host_ip: None, host_port: None });
            }
            for b in list {
                let hp = b.get("HostPort").and_then(Value::as_str).and_then(|p| p.parse().ok());
                let hip = b.get("HostIp").and_then(Value::as_str).map(str::to_string).filter(|x| !x.is_empty());
                // Docker lists IPv4 and IPv6 bindings of one publish separately.
                if ports.iter().any(|p: &PortMap| p.port == port && p.host_port == hp && p.proto == proto) {
                    continue;
                }
                ports.push(PortMap { port, proto: proto.into(), host_ip: hip, host_port: hp });
            }
        }
    }
    ports.sort_by(|a, b| a.port.cmp(&b.port).then(a.host_port.cmp(&b.host_port)));
    let (mut ip, mut gateway) = (None, None);
    if let Some(nets) = v.pointer("/NetworkSettings/Networks").and_then(Value::as_object) {
        for (_, n) in nets {
            let a = n.get("IPAddress").and_then(Value::as_str).unwrap_or("");
            if !a.is_empty() && ip.is_none() {
                ip = Some(a.to_string());
                gateway = n.get("Gateway").and_then(Value::as_str).map(str::to_string).filter(|g| !g.is_empty());
            }
        }
    }
    let mounts = v
        .pointer("/Mounts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| MountInfo {
                    kind: m.get("Type").and_then(Value::as_str).unwrap_or("").into(),
                    source: m.get("Source").and_then(Value::as_str).unwrap_or("").into(),
                    destination: m.get("Destination").and_then(Value::as_str).unwrap_or("").into(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ContainerInfo {
        name: s("/Name").trim_start_matches('/').to_string(),
        status: s("/State/Status"),
        running: v.pointer("/State/Running").and_then(Value::as_bool).unwrap_or(false),
        image: s("/Config/Image"),
        created: s("/Created"),
        ports,
        ip,
        gateway,
        network_mode: s("/HostConfig/NetworkMode"),
        labels,
        env,
        mounts,
        user: s("/Config/User"),
        id,
    })
}

/// Containers with a `devcontainer.local_folder` label: `(id, local folder)`.
pub async fn list_devcontainers(docker: &str) -> Result<Vec<(String, String)>, String> {
    let out = run(
        docker,
        &["ps", "-a", "--no-trunc", "--filter", "label=devcontainer.local_folder", "--format", "{{.ID}}\t{{.Label \"devcontainer.local_folder\"}}"],
        Duration::from_secs(10),
    )
    .await?;
    if !out.ok() {
        return Err(first_line(&out.message()));
    }
    Ok(out
        .stdout
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, folder)| (id.trim().to_string(), folder.trim().to_string()))
        .filter(|(id, _)| !id.is_empty())
        .collect())
}

pub async fn inspect(docker: &str, ids: &[String]) -> Result<Vec<ContainerInfo>, String> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut args: Vec<&str> = vec!["inspect", "--type", "container"];
    args.extend(ids.iter().map(String::as_str));
    let out = run(docker, &args, Duration::from_secs(10)).await?;
    // A container removed between ps and inspect makes inspect fail for that one only.
    let v: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Array(vec![]));
    Ok(v.as_array().map(|a| a.iter().filter_map(parse_inspect).collect()).unwrap_or_default())
}

/// Run `argv` in the container (no TTY) and capture its output.
pub async fn exec(docker: &str, container: &str, user: Option<&str>, argv: &[&str], timeout: Duration) -> Result<Output, String> {
    let mut args: Vec<&str> = vec!["exec"];
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        args.extend(["-u", u]);
    }
    args.push(container);
    args.extend(argv);
    run(docker, &args, timeout).await
}

/// Write `data` to `path` inside the container as `user`, mode 0600, through stdin.
pub async fn write_file(docker: &str, container: &str, user: Option<&str>, path: &str, data: &[u8]) -> Result<(), String> {
    let dir = Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_else(|| "/tmp".into());
    let script = "umask 077; mkdir -p \"$1\" && cat > \"$2.tmp\" && mv -f \"$2.tmp\" \"$2\"";
    let mut args: Vec<&str> = vec!["exec", "-i"];
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        args.extend(["-u", u]);
    }
    args.extend([container, "/bin/sh", "-c", script, "sh", &dir, path]);
    let out = run_stdin(docker, &args, data, Duration::from_secs(15)).await?;
    if out.ok() { Ok(()) } else { Err(first_line(&out.message())) }
}

/// The gateway of a docker network (`bridge` → usually 172.17.0.1).
pub async fn network_gateway(docker: &str, network: &str) -> Option<String> {
    let out = run(docker, &["network", "inspect", network, "--format", "{{range .IPAM.Config}}{{.Gateway}} {{end}}"], Duration::from_secs(8))
        .await
        .ok()?;
    out.stdout.split_whitespace().find(|g| g.parse::<std::net::Ipv4Addr>().is_ok()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inspect_output() {
        let v = json!({
            "Id": "0123456789abcdef0123",
            "Name": "/wbdc-app-1234",
            "Created": "2026-09-26T10:00:00Z",
            "State": {"Status": "running", "Running": true},
            "Config": {"Image": "debian:bookworm-slim", "User": "", "Labels": {"devcontainer.local_folder": "/home/u/app"}, "Env": ["PATH=/usr/bin:/bin", "A=b=c"]},
            "HostConfig": {"NetworkMode": "bridge"},
            "NetworkSettings": {
                "Ports": {"8000/tcp": [{"HostIp": "127.0.0.1", "HostPort": "8000"}], "9000/tcp": null},
                "Networks": {"bridge": {"IPAddress": "172.17.0.2", "Gateway": "172.17.0.1"}}
            },
            "Mounts": [{"Type": "bind", "Source": "/home/u/app", "Destination": "/workspaces/app"}]
        });
        let c = parse_inspect(&v).unwrap();
        assert_eq!(c.name, "wbdc-app-1234");
        assert!(c.running);
        assert_eq!(c.short_id(), "0123456789ab");
        assert_eq!(c.env.get("A").map(String::as_str), Some("b=c"));
        assert_eq!(c.ports.len(), 2);
        assert_eq!(c.ports[0].host_port, Some(8000));
        assert_eq!(c.ports[1].host_port, None);
        assert_eq!(c.ip.as_deref(), Some("172.17.0.2"));
        assert_eq!(c.gateway.as_deref(), Some("172.17.0.1"));
        assert_eq!(c.label("devcontainer.local_folder"), Some("/home/u/app"));
        assert_eq!(c.mounts[0].destination, "/workspaces/app");
    }
}
