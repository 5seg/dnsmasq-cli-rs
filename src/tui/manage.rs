//! 管理操作: ホワイトリスト追加と dnsmasq リロード。
//!
//! すべて `sudo -n` (非対話) で実行する。パスワードレス sudo を前提とし、
//! パスワードが必要な環境では即座に失敗してステータス行に表示する (TUI を固めない)。

use std::io::Write;
use std::process::{Command, Stdio};

use super::Action;

pub const WHITELIST: &str = "/etc/dnsmasq-cli-rs/whitelist.txt";
pub const ADBLOCK: &str = "/etc/dnsmasq.d/adblock.conf";
const RC_SERVICE: &str = "/sbin/rc-service";

pub fn execute(action: &Action) -> String {
    match action {
        Action::Reload => match reload() {
            Ok(()) => "dnsmasq reloaded".to_string(),
            Err(e) => format!("reload failed: {e}"),
        },
        Action::Whitelist(domain) => match whitelist(domain) {
            Ok(msg) => msg,
            Err(e) => format!("whitelist failed: {e}"),
        },
    }
}

fn whitelist(domain: &str) -> Result<String, String> {
    if !is_valid_domain(domain) {
        return Err(format!("invalid domain: {domain}"));
    }
    if !in_whitelist(domain)? {
        append_whitelist(domain)?;
    }
    remove_adblock(domain)?;
    reload()?;
    Ok(format!("whitelisted {domain} + reloaded dnsmasq"))
}

fn reload() -> Result<(), String> {
    run(Command::new("sudo").args(["-n", RC_SERVICE, "dnsmasq", "restart"]))?;
    Ok(())
}

fn in_whitelist(domain: &str) -> Result<bool, String> {
    let status = Command::new("sudo")
        .args(["-n", "grep", "-qxF", "--", domain, WHITELIST])
        .status()
        .map_err(|e| format!("grep spawn: {e}"))?;
    Ok(status.success())
}

fn append_whitelist(domain: &str) -> Result<(), String> {
    let mut child = Command::new("sudo")
        .args(["-n", "tee", "-a", WHITELIST])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| format!("tee spawn: {e}"))?;
    if let Some(mut sin) = child.stdin.take() {
        sin.write_all(domain.as_bytes())
            .and_then(|_| sin.write_all(b"\n"))
            .map_err(|e| format!("tee write: {e}"))?;
    }
    let status = child.wait().map_err(|e| format!("tee wait: {e}"))?;
    if !status.success() {
        return Err("tee failed".to_string());
    }
    Ok(())
}

fn remove_adblock(domain: &str) -> Result<(), String> {
    let escaped = domain.replace('.', "\\.");
    let script = format!("\\|address=/{escaped}/|d");
    run(Command::new("sudo").args(["-n", "sed", "-i", "-E", &script, ADBLOCK]))?;
    Ok(())
}

fn run(cmd: &mut Command) -> Result<std::process::ExitStatus, String> {
    let status = cmd
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("spawn: {e}"))?;
    if !status.success() {
        return Err(format!("exit status {status}"));
    }
    Ok(status)
}

fn is_valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && !d.starts_with('-')
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}
