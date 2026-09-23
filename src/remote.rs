//! Connections to other machines over SSH.
//!
//! A connection installs or updates receipts on the remote machine, starts `receipts serve` there,
//! bound to its 127.0.0.1, and opens an SSH tunnel to it. The local server then forwards API
//! requests through the tunnel (see `machines.rs`), so the remote chats never land on this
//! machine's disk. All SSH commands for a machine share one login through SSH connection sharing
//! (ControlMaster).

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use base64::Engine;

use crate::update;

/// Reports progress while a connection starts, such as "installing receipts on devbox".
pub type Log<'a> = &'a (dyn Fn(String) + Send + Sync);

const INSTALL_SH: &str = "https://github.com/joel-huang/receipts/releases/latest/download/install.sh";
const INSTALL_PS1: &str = "https://github.com/joel-huang/receipts/releases/latest/download/install.ps1";

/// Finds receipts on a POSIX machine. Non-interactive SSH shells often lack ~/.local/bin, where
/// install.sh puts it, in their PATH.
const FIND_POSIX: &str = r#"R=$(command -v receipts || echo "$HOME/.local/bin/receipts")"#;

/// Finds receipts on a Windows machine. install.ps1 puts it in %LOCALAPPDATA%\receipts\bin.
const FIND_POWERSHELL: &str = r#"$R = (Get-Command receipts -ErrorAction SilentlyContinue).Source; if (-not $R) { $R = Join-Path $env:LOCALAPPDATA 'receipts\bin\receipts.exe' }"#;

struct Ssh {
    target: String,
    /// Extra options from the user, such as `-p 2222`.
    args: Vec<String>,
    control_path: PathBuf,
    /// The remote machine runs Windows, so remote commands must be PowerShell.
    windows: bool,
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

    /// Picks the POSIX or the PowerShell version of a remote command. The PowerShell version runs
    /// through `powershell -EncodedCommand`, which works whether the SSH server starts
    /// PowerShell or cmd.exe, and which needs no quoting.
    fn script(&self, posix: String, powershell: String) -> String {
        if !self.windows {
            return posix;
        }
        // Without a terminal, PowerShell writes progress bars and Write-Host messages to stderr as
        // CLIXML. Turn off progress bars, and send Write-Host output (stream 6) to stdout as text.
        let powershell = format!("$ProgressPreference = 'SilentlyContinue'; & {{ {powershell} }} 6>&1");
        let utf16: Vec<u8> = powershell.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!(
            "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
            base64::engine::general_purpose::STANDARD.encode(utf16)
        )
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

/// A running remote server and the SSH tunnel to it. Dropping it stops the remote server and
/// closes the SSH connection.
pub struct Connection {
    ssh: Ssh,
    server: Child,
    /// Local end of the tunnel to the remote server.
    pub port: u16,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        self.ssh.close();
    }
}

/// Connects to `target`, an SSH target such as `user@host` or a Host from ~/.ssh/config. This
/// blocks for as long as the SSH steps take, so call it off the async runtime.
pub fn connect(target: &str, ssh_args: &[String], log: Log) -> anyhow::Result<Connection> {
    // Each connection gets its own socket. The path stays short, because socket paths have a
    // length limit of about 100 characters.
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let mut ssh = Ssh {
        target: target.to_string(),
        args: ssh_args.to_vec(),
        control_path: std::env::temp_dir().join(format!(
            "receipts-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::SeqCst)
        )),
        windows: false,
    };
    match start(&mut ssh, log) {
        Ok((server, port)) => Ok(Connection { ssh, server, port }),
        Err(e) => {
            ssh.close();
            Err(e)
        }
    }
}

fn start(ssh: &mut Ssh, log: Log) -> anyhow::Result<(Child, u16)> {
    log(format!("Connecting to {}", ssh.target));
    ssh.windows = detect_windows(ssh)?;
    let version = ensure_remote_version(ssh, log)?;
    let target = &ssh.target;
    log(format!("Starting receipts {version} on {target}"));

    // On POSIX, -tt gives the remote command a terminal, so it stops when this SSH session ends.
    // Windows OpenSSH ends the session's processes itself, and a Windows terminal session would
    // mix screen control codes into the output that read_remote_port reads.
    // RECEIPTS_NO_UPDATE keeps the remote update prompt from waiting for an answer.
    let mut server = ssh.command();
    if !ssh.windows {
        server.arg("-tt");
    }
    let mut server = server
        .arg(target)
        .arg(ssh.script(
            format!(r#"{FIND_POSIX}; RECEIPTS_NO_UPDATE=1 exec "$R" serve --no-open"#),
            format!(r#"{FIND_POWERSHELL}; $env:RECEIPTS_NO_UPDATE = '1'; & $R serve --no-open"#),
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("run ssh")?;
    let remote_port = match read_remote_port(&mut server, target) {
        Ok(p) => p,
        Err(e) => {
            let _ = server.kill();
            return Err(e);
        }
    };

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
        let _ = server.kill();
        bail!("could not open an ssh tunnel to {target}");
    }
    if let Err(e) = wait_for_port(tunnel_port) {
        let _ = server.kill();
        return Err(e);
    }
    Ok((server, tunnel_port))
}

// ---- remote version ----------------------------------------------------------------------------

/// Installs receipts on the remote machine if it is missing, and updates it if it is older than
/// this machine's version. The local web UI then never asks the remote API for fields that it
/// lacks. Returns the remote version, such as "0.2.3".
fn ensure_remote_version(ssh: &Ssh, log: Log) -> anyhow::Result<String> {
    let target = &ssh.target;
    let local = env!("CARGO_PKG_VERSION");
    let version = match remote_version(ssh)? {
        None => {
            log(format!("Installing receipts on {target}"));
            install_remote(ssh)?;
            remote_version(ssh)?.with_context(|| format!("receipts is still missing on {target} after the install"))?
        }
        Some(v) if older(&v, local) => {
            log(format!("Updating receipts on {target} from {v} to {local}"));
            // `receipts update` exists from 0.1.1 on. Older versions need the installer.
            let update = ssh.script(
                format!(r#"{FIND_POSIX}; "$R" update"#),
                format!(r#"{FIND_POWERSHELL}; & $R update; exit $LASTEXITCODE"#),
            );
            if !ssh.run(&update)? {
                install_remote(ssh)?;
            }
            remote_version(ssh)?.unwrap_or(v)
        }
        Some(v) => v,
    };
    if older(&version, local) {
        log(format!("{target} still has receipts {version}, older than {local}. Some views may not work."));
    }
    Ok(version)
}

fn older(version: &str, than: &str) -> bool {
    matches!((update::parse_version(version), update::parse_version(than)), (Some(a), Some(b)) if a < b)
}

/// Tells a Windows machine from a POSIX one. PowerShell prints `Windows_NT` for this command,
/// cmd.exe prints it unchanged, and a POSIX shell prints `:OS`.
fn detect_windows(ssh: &Ssh) -> anyhow::Result<bool> {
    let out = ssh.output("echo $env:OS")?;
    if !out.status.success() {
        bail!("could not connect to {} over ssh", ssh.target);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.contains("Windows_NT") || text.trim() == "$env:OS")
}

/// Returns the remote receipts version, such as "0.2.3", or None if it is not installed.
fn remote_version(ssh: &Ssh) -> anyhow::Result<Option<String>> {
    let out = ssh.output(&ssh.script(
        format!(r#"{FIND_POSIX}; if [ -x "$R" ]; then "$R" --version; else echo MISSING; fi"#),
        format!(r#"{FIND_POWERSHELL}; if (Test-Path $R) {{ & $R --version }} else {{ 'MISSING' }}"#),
    ))?;
    if !out.status.success() {
        bail!("could not connect to {} over ssh", ssh.target);
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // `receipts --version` prints "receipts 0.2.3".
    Ok((text != "MISSING" && !text.is_empty()).then(|| text.trim_start_matches("receipts ").to_string()))
}

/// Installs receipts on the remote machine with install.sh, or with install.ps1 on Windows.
fn install_remote(ssh: &Ssh) -> anyhow::Result<()> {
    let posix = format!("curl -fsSL {INSTALL_SH} | sh");
    let powershell = format!("irm {INSTALL_PS1} | iex");
    let manual = if ssh.windows { &powershell } else { &posix };
    if !ssh.run(&ssh.script(posix.clone(), powershell.clone()))? {
        bail!("the install on {} failed. Install it there with: {manual}", ssh.target);
    }
    Ok(())
}

// ---- remote server and tunnel ------------------------------------------------------------------

/// Reads the remote server's output until it prints "Receipts running at http://127.0.0.1:<port>".
/// Later output, such as the startup scan count, is printed here with the machine's name.
fn read_remote_port(server: &mut Child, target: &str) -> anyhow::Result<u16> {
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
            let target = target.to_string();
            std::thread::spawn(move || {
                for line in lines.map_while(Result::ok) {
                    eprintln!("{target}: {}", line.trim_end_matches('\r'));
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
