//! Self-update from GitHub releases.
//!
//! Once a day, a normal run asks GitHub for the newest release. If it is newer, the user gets a
//! y/n prompt. The update downloads the archive for this platform, checks its SHA-256 against
//! the `.sha256` file from the release, and swaps the new binary in place of the running one.

use std::io::{IsTerminal, Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

const REPO: &str = "joel-huang/receipts";
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// Release asset names use the Rust target triple; build.rs exports it.
const TARGET: &str = env!("RECEIPTS_TARGET");
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Runs before every command except `update`. A failed check prints nothing, so an offline run
/// stays quiet. A failed install after the user says yes prints an error.
pub fn auto_update() {
    if !auto_enabled() || !check_due() {
        return;
    }
    let checker = agent(Duration::from_secs(3));
    let Ok(Some(tag)) = newer_release(&checker) else { return };

    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        eprintln!("receipts: {tag} is available (you have v{CURRENT}). Run `receipts update` to install it.");
        return;
    }
    if !confirm(&format!("receipts: {tag} is available (you have v{CURRENT}). Update now? [y/N] ")) {
        return;
    }
    match install(&agent(Duration::from_secs(60)), &tag) {
        Ok(()) => eprintln!("receipts: updated to {tag}. The new version starts the next time you run receipts."),
        Err(e) => eprintln!("receipts: update failed: {e:#}. Run `receipts update` to retry."),
    }
}

/// `receipts update`: check now and install without asking.
pub fn update_command() -> anyhow::Result<()> {
    let agent = agent(Duration::from_secs(60));
    match newer_release(&agent)? {
        None => println!("receipts v{CURRENT} is the latest version."),
        Some(tag) => {
            println!("Updating v{CURRENT} to {tag}…");
            install(&agent, &tag)?;
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

/// Records the check time before the check runs, so a failed or declined check waits a day too.
fn check_due() -> bool {
    let dir = crate::index::data_dir();
    let stamp = dir.join("last-update-check");
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if now.saturating_sub(last) < CHECK_INTERVAL_SECS {
        return false;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&stamp, now.to_string());
    true
}

fn confirm(question: &str) -> bool {
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes" | "Yes")
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout(timeout)
        .user_agent(concat!("receipts/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// GitHub redirects /releases/latest to /releases/tag/<tag>. Reading the redirect avoids the
/// REST API and its limit of 60 unauthenticated requests an hour.
fn newer_release(agent: &ureq::Agent) -> anyhow::Result<Option<String>> {
    let resp = agent.get(&format!("https://github.com/{REPO}/releases/latest")).call()?;
    let tag = resp
        .get_url()
        .rsplit_once("/tag/")
        .map(|(_, t)| t.to_string())
        .context("the repo has no published release")?;
    let (Some(latest), Some(current)) = (parse_version(&tag), parse_version(CURRENT)) else {
        bail!("cannot compare versions {tag} and v{CURRENT}");
    };
    Ok((latest > current).then_some(tag))
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

fn download(agent: &ureq::Agent, url: &str) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    agent
        .get(url)
        .call()
        .with_context(|| format!("download {url}"))?
        .into_reader()
        .take(200 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn install(agent: &ureq::Agent, tag: &str) -> anyhow::Result<()> {
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset = format!("receipts-{TARGET}.{ext}");
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");

    let archive = download(agent, &format!("{base}/{asset}"))?;
    let sums = String::from_utf8(download(agent, &format!("{base}/{asset}.sha256"))?)?;
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
