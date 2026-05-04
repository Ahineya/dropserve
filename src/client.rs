use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};

use crate::util::{
    artifact_bytes, ensure_parent, parse_user_host, random_upload_name, validate_domain,
    ENV_SSH_KEY,
};

fn ssh_key_from_opts(cli_key: Option<PathBuf>) -> Result<Option<PathBuf>> {
    if let Some(p) = cli_key {
        return Ok(Some(p));
    }
    if let Ok(s) = std::env::var(ENV_SSH_KEY) {
        if s.is_empty() {
            return Ok(None);
        }
        return Ok(Some(PathBuf::from(s)));
    }
    Ok(None)
}

fn push_ssh_args(cmd: &mut std::process::Command, key: Option<&Path>) {
    if let Some(k) = key {
        cmd.arg("-i").arg(k);
    }
}

pub fn create(
    user_host: &str,
    domain: &str,
    local: &Path,
    pocketbase: bool,
    pb_port: u16,
    ssh_key: Option<PathBuf>,
) -> Result<()> {
    validate_domain(domain)?;
    if pocketbase && pb_port == 0 {
        anyhow::bail!("--pb-port must be between 1 and 65535");
    }
    let (user, host) = parse_user_host(user_host)?;
    let spec = format!("{user}@{host}");
    let key = ssh_key_from_opts(ssh_key)?;

    let bytes = artifact_bytes(local)?;
    let bundle = random_upload_name();
    let tmp = std::env::temp_dir().join(&bundle);
    ensure_parent(&tmp)?;
    fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;

    let remote_path = format!("/tmp/{bundle}");

    let mut scp = std::process::Command::new("scp");
    push_ssh_args(&mut scp, key.as_deref());
    scp.arg(&tmp).arg(format!("{spec}:{remote_path}"));
    let st = scp.status().context("spawn scp")?;
    if !st.success() {
        anyhow::bail!("scp failed with status {st}");
    }
    let _ = fs::remove_file(&tmp);

    let mut ssh = std::process::Command::new("ssh");
    push_ssh_args(&mut ssh, key.as_deref());
    ssh.arg(&spec)
        .arg("dropserve")
        .arg("remote")
        .arg("create")
        .arg(domain)
        .arg(&remote_path);
    if pocketbase {
        ssh.arg("--pocketbase")
            .arg("--pb-port")
            .arg(pb_port.to_string());
    }
    let st = ssh.status().context("spawn ssh")?;
    if !st.success() {
        anyhow::bail!("remote create failed with status {st}");
    }
    Ok(())
}

pub fn update(user_host: &str, domain: &str, local: &Path, ssh_key: Option<PathBuf>) -> Result<()> {
    validate_domain(domain)?;
    let (user, host) = parse_user_host(user_host)?;
    let spec = format!("{user}@{host}");
    let key = ssh_key_from_opts(ssh_key)?;

    let bytes = artifact_bytes(local)?;
    let bundle = random_upload_name();
    let tmp = std::env::temp_dir().join(&bundle);
    ensure_parent(&tmp)?;
    fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;

    let remote_path = format!("/tmp/{bundle}");

    let mut scp = std::process::Command::new("scp");
    push_ssh_args(&mut scp, key.as_deref());
    scp.arg(&tmp).arg(format!("{spec}:{remote_path}"));
    let st = scp.status().context("spawn scp")?;
    if !st.success() {
        anyhow::bail!("scp failed with status {st}");
    }
    let _ = fs::remove_file(&tmp);

    let mut ssh = std::process::Command::new("ssh");
    push_ssh_args(&mut ssh, key.as_deref());
    ssh.arg(&spec)
        .arg("dropserve")
        .arg("remote")
        .arg("update")
        .arg(domain)
        .arg(&remote_path);
    let st = ssh.status().context("spawn ssh")?;
    if !st.success() {
        anyhow::bail!("remote update failed with status {st}");
    }
    Ok(())
}

pub fn delete(user_host: &str, domain: &str, ssh_key: Option<PathBuf>) -> Result<()> {
    validate_domain(domain)?;
    println!("Type DELETE to remove site {domain} on {user_host}:");
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    if line.trim() != "DELETE" {
        anyhow::bail!("aborted");
    }

    let (user, host) = parse_user_host(user_host)?;
    let spec = format!("{user}@{host}");
    let key = ssh_key_from_opts(ssh_key)?;

    let mut ssh = std::process::Command::new("ssh");
    push_ssh_args(&mut ssh, key.as_deref());
    ssh.arg(&spec)
        .arg("dropserve")
        .arg("remote")
        .arg("delete")
        .arg(domain);
    let st = ssh.status().context("spawn ssh")?;
    if !st.success() {
        anyhow::bail!("remote delete failed with status {st}");
    }
    Ok(())
}

pub fn list_sites(user_host: &str, ssh_key: Option<PathBuf>) -> Result<()> {
    let (user, host) = parse_user_host(user_host)?;
    let spec = format!("{user}@{host}");
    let key = ssh_key_from_opts(ssh_key)?;

    let mut ssh = std::process::Command::new("ssh");
    push_ssh_args(&mut ssh, key.as_deref());
    ssh.arg(&spec).arg("dropserve").arg("remote").arg("list");
    ssh.stdin(Stdio::null());
    let st = ssh.status().context("spawn ssh")?;
    if !st.success() {
        anyhow::bail!("remote list failed with status {st}");
    }
    Ok(())
}

pub fn rollback(user_host: &str, domain: &str, ssh_key: Option<PathBuf>) -> Result<()> {
    validate_domain(domain)?;
    let (user, host) = parse_user_host(user_host)?;
    let spec = format!("{user}@{host}");
    let key = ssh_key_from_opts(ssh_key)?;

    let mut ssh = std::process::Command::new("ssh");
    push_ssh_args(&mut ssh, key.as_deref());
    ssh.arg(&spec)
        .arg("dropserve")
        .arg("remote")
        .arg("rollback")
        .arg(domain);
    let st = ssh.status().context("spawn ssh")?;
    if !st.success() {
        anyhow::bail!("remote rollback failed with status {st}");
    }
    Ok(())
}
