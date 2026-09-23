//! `receipts remote <target>`: browse the chats on another machine over SSH.
//!
//! The command starts `receipts serve` on the remote machine, bound to its 127.0.0.1, and opens an
//! SSH tunnel to it. A local server then serves this machine's web UI and forwards `/api/...`
//! requests through the tunnel. The remote chats never land on this machine's disk. All SSH
//! connections share one login through SSH connection sharing (ControlMaster).

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use http_body_util::BodyExt;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;

use crate::{server, update};

const INSTALL_URL: &str = "https://github.com/joel-huang/receipts/releases/latest/download/install.sh";

/// Finds receipts on the remote machine. Non-interactive SSH shells often lack ~/.local/bin, where
/// the installer puts it, in their PATH.
const FIND_RECEIPTS: &str = r#"R=$(command -v receipts || echo "$HOME/.local/bin/receipts")"#;

struct Ssh {
    target: String,
    /// Extra options from the user, such as `-p 2222`.
    args: Vec<String>,
    control_path: PathBuf,
}

impl Ssh {
    fn command(&self) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg(format!("ControlPath={}", self.control_path.display()))
            .arg("-o")
            .arg("ControlPersist=60")
            .args(&self.args);
        cmd
    }

    /// Runs a shell command on the remote machine and returns its standard output.
    fn output(&self, script: &str) -> anyhow::Result<std::process::Output> {
        self.command()
            .arg(&self.target)
            .arg(script)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .context("run ssh (is OpenSSH installed?)")
    }

    /// Runs a shell command on the remote machine and shows its output here.
    fn run(&self, script: &str) -> anyhow::Result<bool> {
        let status = self.command().arg(&self.target).arg(script).status().context("run ssh")?;
        Ok(status.success())
    }

    /// Closes the shared SSH connection.
    fn close(&self) {
        let _ = self
            .command()
            .arg("-O")
            .arg("exit")
            .arg(&self.target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub async fn run(target: String, ssh_args: Vec<String>, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let ssh = Ssh {
        target,
        args: ssh_args,
        control_path: std::env::temp_dir().join(format!("receipts-ssh-{}", std::process::id())),
    };
    let result = serve_remote(&ssh, port, open_browser).await;
    ssh.close();
    result
}

async fn serve_remote(ssh: &Ssh, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let target = &ssh.target;
    let version = ensure_remote_version(ssh)?;
    eprintln!("receipts: {target} has receipts {version}");

    // -tt gives the remote command a terminal, so it stops when this SSH session ends.
    // RECEIPTS_NO_UPDATE keeps the remote update prompt from waiting for an answer.
    let mut server = ssh
        .command()
        .arg("-tt")
        .arg(target)
        .arg(format!(r#"{FIND_RECEIPTS}; RECEIPTS_NO_UPDATE=1 exec "$R" serve --no-open"#))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("run ssh")?;
    let remote_port = read_remote_port(&mut server)?;

    // Add the tunnel to the shared connection. A separate `ssh -N -L` process can hand the
    // forward to the shared connection and exit right away. Closing the connection removes it.
    let tunnel_port = free_local_port()?;
    let forwarded = ssh
        .command()
        .arg("-O")
        .arg("forward")
        .arg("-L")
        .arg(format!("127.0.0.1:{tunnel_port}:127.0.0.1:{remote_port}"))
        .arg(target)
        .stdout(Stdio::null())
        .status()
        .context("run ssh")?;
    if !forwarded.success() {
        bail!("could not open an ssh tunnel to {target}");
    }
    wait_for_port(tunnel_port)?;

    let proxy = Arc::new(Proxy {
        client: Client::builder(TokioExecutor::new()).build_http(),
        upstream: tunnel_port,
        remote: target.clone(),
    });
    let app = Router::new()
        .route("/api/{*path}", any(forward))
        .fallback(server::static_asset)
        .layer(middleware::from_fn(loopback_only))
        .with_state(proxy);
    let listener = server::bind("127.0.0.1", port).await?;
    let url = format!("http://{}", listener.local_addr()?);
    println!("Receipts on {target} running at {url}  (Ctrl+C to stop)");
    if open_browser {
        let _ = open::that(&url);
    }

    // Stop on Ctrl+C, or when the remote server ends.
    let server_done = tokio::task::spawn_blocking(move || server.wait());
    let result = tokio::select! {
        _ = tokio::signal::ctrl_c() => Ok(()),
        served = axum::serve(listener, app) => served.map_err(Into::into),
        status = server_done => match status {
            Ok(Ok(s)) => Err(anyhow::anyhow!("the remote server stopped ({s})")),
            _ => Err(anyhow::anyhow!("the remote server stopped")),
        },
    };
    result
}

// ---- remote version ----------------------------------------------------------------------------

/// Installs receipts on the remote machine if it is missing, and updates it if it is older than
/// this machine's version. The local web UI then never asks the remote API for fields that it
/// lacks. Returns the remote version, such as "0.2.3".
fn ensure_remote_version(ssh: &Ssh) -> anyhow::Result<String> {
    let target = &ssh.target;
    let local = env!("CARGO_PKG_VERSION");
    let version = match remote_version(ssh)? {
        None => {
            eprintln!("receipts: installing receipts on {target}");
            install_remote(ssh)?;
            remote_version(ssh)?.with_context(|| format!("receipts is still missing on {target} after the install"))?
        }
        Some(v) if older(&v, local) => {
            eprintln!("receipts: updating receipts on {target} from {v} to match {local}");
            // `receipts update` exists from 0.1.1 on. Older versions need the installer.
            if !ssh.run(&format!(r#"{FIND_RECEIPTS}; "$R" update"#))? {
                install_remote(ssh)?;
            }
            remote_version(ssh)?.unwrap_or(v)
        }
        Some(v) => v,
    };
    if older(&version, local) {
        eprintln!("receipts: {target} still has {version}, older than {local}. Some views may not work.");
    }
    Ok(version)
}

fn older(version: &str, than: &str) -> bool {
    matches!((update::parse_version(version), update::parse_version(than)), (Some(a), Some(b)) if a < b)
}

/// Returns the remote receipts version, such as "0.2.3", or None if it is not installed.
fn remote_version(ssh: &Ssh) -> anyhow::Result<Option<String>> {
    let out = ssh.output(&format!(r#"{FIND_RECEIPTS}; if [ -x "$R" ]; then "$R" --version; else echo MISSING; fi"#))?;
    if !out.status.success() {
        bail!("could not connect to {} over ssh", ssh.target);
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // `receipts --version` prints "receipts 0.2.3".
    Ok((text != "MISSING" && !text.is_empty()).then(|| text.trim_start_matches("receipts ").to_string()))
}

/// Installs receipts on the remote machine with the curl installer.
fn install_remote(ssh: &Ssh) -> anyhow::Result<()> {
    if !ssh.run(&format!("curl -fsSL {INSTALL_URL} | sh"))? {
        bail!(
            "the install on {} failed. Install it there with: curl -fsSL {INSTALL_URL} | sh",
            ssh.target
        );
    }
    Ok(())
}

// ---- remote server and tunnel ------------------------------------------------------------------

/// Reads the remote server's output until it prints "Receipts running at http://127.0.0.1:<port>".
/// Later output, such as the startup scan count, is passed through with a `remote:` prefix.
fn read_remote_port(server: &mut Child) -> anyhow::Result<u16> {
    let stdout = server.stdout.take().context("read the remote server output")?;
    let mut lines = BufReader::new(stdout).lines();
    let marker = "Receipts running at http://127.0.0.1:";
    let mut seen = Vec::new();
    for line in lines.by_ref() {
        let line = line?;
        let line = line.trim_end_matches('\r').to_string();
        if let Some(rest) = line.split_once(marker).map(|(_, r)| r) {
            let port: u16 = rest
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or("")
                .parse()
                .with_context(|| format!("read the port from: {line}"))?;
            std::thread::spawn(move || {
                for line in lines.map_while(Result::ok) {
                    eprintln!("remote: {}", line.trim_end_matches('\r'));
                }
            });
            return Ok(port);
        }
        seen.push(line);
    }
    bail!("the remote server did not start:\n{}", seen.join("\n"))
}

fn free_local_port() -> anyhow::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Waits until the tunnel accepts connections on the local port.
fn wait_for_port(port: u16) -> anyhow::Result<()> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("the ssh tunnel did not open local port {port}")
}

// ---- local server ------------------------------------------------------------------------------

struct Proxy {
    client: Client<HttpConnector, Body>,
    /// Local end of the SSH tunnel to the remote server.
    upstream: u16,
    /// SSH target, which the web UI shows next to file paths.
    remote: String,
}

async fn loopback_only(req: Request, next: Next) -> Response {
    match server::host_rejection(&req, &[]) {
        None => next.run(req).await,
        Some(resp) => resp,
    }
}

/// Forwards an API request through the tunnel. The status response also gets a `remote` field,
/// so the web UI can mark file paths as remote.
async fn forward(State(proxy): State<Arc<Proxy>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let path_and_query = req.uri().path_and_query().map_or("/", |p| p.as_str()).to_string();
    let upstream = format!("127.0.0.1:{}", proxy.upstream);
    let (mut parts, body) = req.into_parts();
    parts.uri = match format!("http://{upstream}{path_and_query}").parse::<Uri>() {
        Ok(uri) => uri,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    // The remote server only answers loopback hostnames.
    parts.headers.insert(header::HOST, HeaderValue::from_str(&upstream).expect("valid host"));

    let resp = match proxy.client.request(Request::from_parts(parts, body)).await {
        Ok(resp) => resp,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("the remote server did not answer: {e}")).into_response(),
    };
    if path != "/api/status" || !resp.status().is_success() {
        return resp.map(Body::new).into_response();
    }
    let bytes = match resp.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let mut status: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    if let Some(obj) = status.as_object_mut() {
        obj.insert("remote".into(), proxy.remote.clone().into());
    }
    axum::Json(status).into_response()
}
