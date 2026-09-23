//! Self-update from GitHub releases.
//!
//! Every normal run asks GitHub for the newest release. If it is newer, the user gets a y/n
//! prompt. The update downloads the archive for this platform, checks its SHA-256 against the
//! `.sha256` file from the release, and swaps the new binary in place of the running one.

use std::io::{IsTerminal, Read, Write};
use std::time::Duration;

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

const REPO: &str = "joel-huang/receipts";
/// Release asset names use the Rust target triple; build.rs exports it.
const TARGET: &str = env!("RECEIPTS_TARGET");
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Runs before every command except `update`. A failed check prints nothing, so an offline run
/// stays quiet. A failed install after the user says yes prints an error.
pub fn auto_update() {
    if !auto_enabled() {
        return;
    }
    let Ok(tag) = latest_tag(Duration::from_secs(3)) else { return };
    if !is_newer(&tag) {
        return;
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        eprintln!("receipts: {tag} is available (you have v{CURRENT}). Run `receipts update` to install it.");
        return;
    }
    if !confirm(&format!("receipts: {tag} is available (you have v{CURRENT}). Update now? [y/N] ")) {
        return;
    }
    match install(&tag) {
        Ok(()) => {
            // The old version is still running. Exit so that the next run uses the new version.
            eprintln!("receipts: updated to {tag}. Run `receipts` again to start it.");
            std::process::exit(0);
        }
        Err(e) => eprintln!("receipts: update failed: {e:#}. Run `receipts update` to retry."),
    }
}

/// `receipts update`: check now and install without asking.
pub fn update_command() -> anyhow::Result<()> {
    let tag = latest_tag(Duration::from_secs(10))?;
    match is_newer(&tag).then_some(tag) {
        None => println!("receipts v{CURRENT} is the latest version."),
        Some(tag) => {
            println!("Updating v{CURRENT} to {tag}…");
            install(&tag)?;
            println!("Updated to {tag}.");
        }
    }
    Ok(())
}

fn auto_enabled() -> bool {
    if cfg!(debug_assertions) || std::env::var_os("RECEIPTS_NO_UPDATE").is_some() {
        return false;
    }
    // `cargo build --release` puts the binary under target/. Updating it would replace a dev build.
    std::env::current_exe()
        .map(|p| !p.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(false)
}

fn confirm(question: &str) -> bool {
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes" | "Yes")
}

const USER_AGENT: &str = concat!("receipts/", env!("CARGO_PKG_VERSION"));

/// GitHub redirects /releases/latest to /releases/tag/<tag>. The check reads the tag from the
/// redirect and does not follow it, so it skips the release web page. It also avoids the REST
/// API and its limit of 60 unauthenticated requests an hour.
fn latest_tag(timeout: Duration) -> anyhow::Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout(timeout)
        .redirects(0)
        .user_agent(USER_AGENT)
        .build();
    let resp = agent.get(&format!("https://github.com/{REPO}/releases/latest")).call()?;
    resp.header("location")
        .and_then(|l| l.rsplit_once("/tag/"))
        .map(|(_, t)| t.to_string())
        .context("the repo has no published release")
}

/// True if `tag` is a newer version than this binary. Tags that do not parse count as not newer.
fn is_newer(tag: &str) -> bool {
    matches!((parse_version(tag), parse_version(CURRENT)), (Some(latest), Some(current)) if latest > current)
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    // Pre-release suffixes like "1.2.0-beta" compare as "1.2.0".
    let mut parts = v
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("").parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Downloads `url` into memory. With a `progress` label, it shows the size downloaded so far.
fn download(agent: &ureq::Agent, url: &str, progress: Option<&str>) -> anyhow::Result<Vec<u8>> {
    let resp = agent.get(url).call().with_context(|| format!("download {url}"))?;
    let total = resp.header("content-length").and_then(|v| v.parse::<f64>().ok());
    let mut reader = resp.into_reader().take(200 * 1024 * 1024);
    let mut buf = Vec::new();
    let mut chunk = vec![0; 64 * 1024];
    let mb = |bytes: f64| bytes / 1024.0 / 1024.0;
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(label) = progress {
            match total {
                Some(t) => eprint!("\r{label} {:.1} / {:.1} MB", mb(buf.len() as f64), mb(t)),
                None => eprint!("\r{label} {:.1} MB", mb(buf.len() as f64)),
            }
        }
    }
    if progress.is_some() {
        eprintln!();
    }
    Ok(buf)
}

fn install(tag: &str) -> anyhow::Result<()> {
    // No limit on the total time, so a slow connection can finish. A download fails only if no
    // data arrives for 30 seconds.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build();
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset = format!("receipts-{TARGET}.{ext}");
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");

    let label = format!("receipts: downloading {tag}…");
    let progress = std::io::stderr().is_terminal().then_some(label.as_str());
    let archive = download(&agent, &format!("{base}/{asset}"), progress)?;
    let sums = String::from_utf8(download(&agent, &format!("{base}/{asset}.sha256"), None)?)?;
    let expected = sums.split_whitespace().next().context("empty checksum file")?.to_lowercase();
    let actual = format!("{:x}", Sha256::digest(&archive));
    if expected != actual {
        bail!("checksum mismatch for {asset}");
    }

    let binary = extract(&archive)?;
    let tmp = std::env::temp_dir().join(format!("receipts-update-{}", std::process::id()));
    std::fs::write(&tmp, binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    // self_replace copies the file next to the running binary and renames it into place.
    // On Windows it also handles the lock that the OS keeps on a running .exe.
    let result = self_replace::self_replace(&tmp).context("replace the receipts binary (is its folder writable?)");
    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(not(windows))]
fn extract(archive: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.path()?.file_name().is_some_and(|n| n == "receipts") {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("the archive has no receipts binary")
}

#[cfg(windows)]
fn extract(archive: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
    let mut file = zip.by_name("receipts.exe")?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("v0.1.2"), Some((0, 1, 2)));
        assert_eq!(parse_version("1.10.0-beta.1"), Some((1, 10, 0)));
        assert_eq!(parse_version("latest"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }
}
