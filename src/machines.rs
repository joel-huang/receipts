//! Other machines that the web app can browse over SSH.
//!
//! The list comes from the Host entries in ~/.ssh/config. A background task checks every 30
//! seconds whether each machine's SSH server answers. That check opens a TCP connection and reads
//! the SSH greeting, and it needs no login. When the web app picks a machine, the server connects
//! to it (see `remote.rs`) and forwards `/api/m/<machine>/...` through the SSH tunnel.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use http_body_util::BodyExt;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde::Serialize;
use serde_json::json;

use crate::remote;

const CHECK_EVERY: Duration = Duration::from_secs(30);
/// Git hosting services accept SSH but run no shell, so receipts cannot run there.
const GIT_HOSTS: &[&str] = &["github.com", "gitlab.com", "bitbucket.org", "ssh.dev.azure.com", "vs-ssh.visualstudio.com"];
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
enum Link {
    #[default]
    Idle,
    /// Connecting, with the latest progress message.
    Connecting(String),
    Connected(remote::Connection),
    Failed(String),
}

#[derive(Default)]
struct Machine {
    /// Whether the SSH server answered the last check. None means that the check could not tell,
    /// for example for a machine behind a jump host.
    reachable: Option<bool>,
    /// SSH greeting, such as "OpenSSH_for_Windows_9.5", or why the check failed.
    detail: Option<String>,
    link: Link,
}

/// What the web app sees of a machine.
#[derive(Serialize)]
pub struct MachineInfo {
    name: String,
    reachable: Option<bool>,
    detail: Option<String>,
    /// idle | connecting | connected | failed
    link: &'static str,
    message: Option<String>,
}

pub struct Machines {
    machines: Mutex<HashMap<String, Machine>>,
    client: Client<HttpConnector, Body>,
}

pub type SharedMachines = Arc<Machines>;

impl Machines {
    pub fn new() -> SharedMachines {
        Arc::new(Machines {
            machines: Mutex::new(HashMap::new()),
            client: Client::builder(TokioExecutor::new()).build_http(),
        })
    }

    /// Checks every machine now and then every 30 seconds.
    pub fn start_checks(self: &SharedMachines) {
        let machines = self.clone();
        tokio::spawn(async move {
            loop {
                let m = machines.clone();
                let _ = tokio::task::spawn_blocking(move || m.check_all()).await;
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    fn check_all(&self) {
        let hosts = ssh_hosts();
        {
            let mut machines = self.machines.lock().unwrap();
            // Keep machines that are connected even if they left the config.
            machines.retain(|name, m| hosts.contains(name) || matches!(m.link, Link::Connected(_)));
            for h in &hosts {
                machines.entry(h.clone()).or_default();
            }
        }
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = hosts.iter().map(|h| scope.spawn(move || (h.clone(), check(h)))).collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        let mut machines = self.machines.lock().unwrap();
        for (name, result) in results {
            match result {
                Some((reachable, detail)) => {
                    if let Some(m) = machines.get_mut(&name) {
                        m.reachable = reachable;
                        m.detail = detail;
                    }
                }
                None => {
                    machines.remove(&name);
                }
            }
        }
    }

    pub fn list(&self) -> Vec<MachineInfo> {
        let machines = self.machines.lock().unwrap();
        let mut list: Vec<_> = machines.iter().map(|(name, m)| info(name, m)).collect();
        list.sort_by_key(|m| m.name.to_lowercase());
        list
    }

    /// Starts connecting to a machine in the background, unless it is connected or connecting.
    pub fn connect(self: &SharedMachines, name: &str) -> Option<MachineInfo> {
        {
            let mut machines = self.machines.lock().unwrap();
            let m = machines.get_mut(name)?;
            if matches!(m.link, Link::Connected(_) | Link::Connecting(_)) {
                return Some(info(name, m));
            }
            m.link = Link::Connecting(format!("Connecting to {name}"));
        }
        let machines = self.clone();
        let target = name.to_string();
        tokio::task::spawn_blocking(move || {
            let log = |msg: String| {
                eprintln!("receipts: {msg}");
                if let Some(m) = machines.machines.lock().unwrap().get_mut(&target) {
                    if matches!(m.link, Link::Connecting(_)) {
                        m.link = Link::Connecting(msg);
                    }
                }
            };
            let result = remote::connect(&target, &ssh_args(), &log);
            let mut all = machines.machines.lock().unwrap();
            if let Some(m) = all.get_mut(&target) {
                m.link = match result {
                    Ok(conn) => {
                        eprintln!("receipts: connected to {target}");
                        Link::Connected(conn)
                    }
                    Err(e) => {
                        eprintln!("receipts: could not connect to {target}: {e:#}");
                        Link::Failed(format!("{e:#}"))
                    }
                };
            }
        });
        let machines = self.machines.lock().unwrap();
        machines.get(name).map(|m| info(name, m))
    }

    /// Stops every remote server and closes every SSH connection.
    pub fn disconnect_all(&self) {
        let links: Vec<Link> = self
            .machines
            .lock()
            .unwrap()
            .values_mut()
            .map(|m| std::mem::take(&mut m.link))
            .collect();
        drop(links);
    }

    fn port(&self, name: &str) -> Option<u16> {
        match &self.machines.lock().unwrap().get(name)?.link {
            Link::Connected(conn) => Some(conn.port),
            _ => None,
        }
    }

    fn lost(&self, name: &str) {
        if let Some(m) = self.machines.lock().unwrap().get_mut(name) {
            m.link = Link::Failed(format!("Lost the connection to {name}"));
        }
    }
}

fn info(name: &str, m: &Machine) -> MachineInfo {
    let (link, message) = match &m.link {
        Link::Idle => ("idle", None),
        Link::Connecting(msg) => ("connecting", Some(msg.clone())),
        Link::Connected(_) => ("connected", None),
        Link::Failed(msg) => ("failed", Some(msg.clone())),
    };
    MachineInfo { name: name.to_string(), reachable: m.reachable, detail: m.detail.clone(), link, message }
}

// ---- finding and checking machines -------------------------------------------------------------

/// SSH config file from RECEIPTS_SSH_CONFIG, which replaces ~/.ssh/config for receipts.
fn custom_config() -> Option<PathBuf> {
    std::env::var_os("RECEIPTS_SSH_CONFIG").map(PathBuf::from)
}

/// Options that every ssh command needs, such as `-F <file>` for RECEIPTS_SSH_CONFIG.
fn ssh_args() -> Vec<String> {
    custom_config().map(|c| vec!["-F".into(), c.to_string_lossy().to_string()]).unwrap_or_default()
}

/// Host names from the SSH config and the files that it includes. Wildcard entries such as
/// `Host *` are settings for many hosts, not machines, so they are left out.
fn ssh_hosts() -> Vec<String> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let config = custom_config().unwrap_or_else(|| home.join(".ssh/config"));
    let mut hosts = Vec::new();
    read_config(&config, &home.join(".ssh"), &mut hosts, 0);
    hosts
}

fn read_config(path: &Path, ssh_dir: &Path, hosts: &mut Vec<String>, depth: usize) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.trim();
        let (key, rest) = line.split_once(|c: char| c.is_whitespace() || c == '=').unwrap_or((line, ""));
        let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                for name in rest.split_whitespace() {
                    if !name.contains(['*', '?', '!']) && !hosts.iter().any(|h| h == name) {
                        hosts.push(name.to_string());
                    }
                }
            }
            "include" if depth < 5 => {
                for pattern in rest.split_whitespace() {
                    for file in include_files(pattern, ssh_dir) {
                        read_config(&file, ssh_dir, hosts, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Files for an Include pattern. Relative paths start in ~/.ssh. Only a `*` in the file name is
/// supported, which covers the common `Include config.d/*`.
fn include_files(pattern: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let expanded = match pattern.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => ssh_dir.join(pattern),
    };
    let name = expanded.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !name.contains('*') {
        return vec![expanded];
    }
    let (prefix, suffix) = name.split_once('*').unwrap_or((&name, ""));
    let Some(dir) = expanded.parent() else { return vec![] };
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            p.is_file() && n.starts_with(prefix) && n.ends_with(suffix)
        })
        .collect();
    files.sort();
    files
}

/// Checks whether the machine's SSH server answers, without logging in. `ssh -G` prints the
/// settings that ssh would use, without connecting. Returns None for git hosting services.
fn check(host: &str) -> Option<(Option<bool>, Option<String>)> {
    let Ok(out) = Command::new("ssh").args(ssh_args()).arg("-G").arg(host).output() else {
        return Some((None, Some("ssh is not installed".into())));
    };
    let config = String::from_utf8_lossy(&out.stdout);
    let value = |key: &str| {
        config
            .lines()
            .find_map(|l| l.split_once(' ').filter(|(k, _)| *k == key).map(|(_, v)| v.trim().to_string()))
    };
    let hostname = value("hostname").unwrap_or_else(|| host.to_string());
    if GIT_HOSTS.iter().any(|g| hostname == *g || hostname.ends_with(&format!(".{g}"))) {
        return None;
    }
    if value("proxyjump").is_some_and(|v| v != "none") || value("proxycommand").is_some_and(|v| v != "none") {
        return Some((None, Some("behind a jump host".into())));
    }
    let port: u16 = value("port").and_then(|p| p.parse().ok()).unwrap_or(22);
    let Some(addr) = (hostname.as_str(), port).to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return Some((Some(false), Some(format!("cannot resolve {hostname}"))));
    };
    Some(match TcpStream::connect_timeout(&addr, CHECK_TIMEOUT) {
        Ok(stream) => {
            let _ = stream.set_read_timeout(Some(CHECK_TIMEOUT));
            let mut greeting = String::new();
            let _ = BufReader::new(stream).read_line(&mut greeting);
            // The greeting reads like "SSH-2.0-OpenSSH_for_Windows_9.5".
            let software = greeting.trim().strip_prefix("SSH-2.0-").map(str::to_string);
            (Some(true), software)
        }
        Err(e) => (Some(false), Some(e.to_string())),
    })
}

// ---- forwarding --------------------------------------------------------------------------------

/// Forwards `/api/m/<machine>/<rest>` to `/api/<rest>` on the machine's remote server. The status
/// response also gets a `remote` field, so the web app can mark file paths as remote.
pub async fn forward(machines: &Machines, name: &str, rest: &str, req: Request) -> Response {
    let Some(port) = machines.port(name) else {
        return (StatusCode::CONFLICT, Json(json!({ "error": format!("not connected to {name}") }))).into_response();
    };
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let upstream = format!("127.0.0.1:{port}");
    let is_status = rest == "status";
    let (mut parts, body) = req.into_parts();
    parts.uri = match format!("http://{upstream}/api/{rest}{query}").parse::<Uri>() {
        Ok(uri) => uri,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    // The remote server only answers loopback hostnames.
    parts.headers.insert(header::HOST, HeaderValue::from_str(&upstream).expect("valid host"));
    // The status response gets a field added below, so it must arrive uncompressed. Other
    // responses pass through compressed, which keeps them small over the SSH link.
    if is_status {
        parts.headers.remove(header::ACCEPT_ENCODING);
    }
    let resp = match machines.client.request(Request::from_parts(parts, body)).await {
        Ok(resp) => resp,
        Err(e) => {
            machines.lost(name);
            return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("{name} did not answer: {e}") }))).into_response();
        }
    };
    if !is_status || !resp.status().is_success() {
        return resp.map(Body::new).into_response();
    }
    let bytes = match resp.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let mut status: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    if let Some(obj) = status.as_object_mut() {
        obj.insert("remote".into(), name.into());
    }
    Json(status).into_response()
}
