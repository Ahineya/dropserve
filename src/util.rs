use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use rand::Rng;
use regex::Regex;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Environment variable for path to SSH private key (optional).
pub const ENV_SSH_KEY: &str = "DROPSERVE_SSH_KEY";

/// Environment variable for dropserve home on server (optional).
pub const ENV_HOME: &str = "DROPSERVE_HOME";

pub fn default_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dropserve")
}

fn domain_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[a-zA-Z0-9]([a-zA-Z0-9.-]*[a-zA-Z0-9])?$|^[a-zA-Z0-9]$").expect("valid regex")
    })
}

pub fn validate_domain(domain: &str) -> Result<()> {
    if !domain_regex().is_match(domain) || domain.len() > 253 {
        anyhow::bail!("invalid domain: {domain}");
    }
    Ok(())
}

pub fn parse_user_host(spec: &str) -> Result<(String, String)> {
    let (user, host) = spec
        .split_once('@')
        .ok_or_else(|| anyhow!("expected user@host, got {spec}"))?;
    if user.is_empty() || host.is_empty() {
        anyhow::bail!("invalid user@host: {spec}");
    }
    Ok((user.to_string(), host.to_string()))
}

pub fn random_upload_name() -> String {
    let n: u64 = rand::thread_rng().gen();
    format!("dropserve-upload-{n:x}.zip")
}

pub fn expand_home(path: &Path) -> PathBuf {
    if path.as_os_str() == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    if let Ok(rest) = path.strip_prefix("~/") {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest);
    }
    path.to_path_buf()
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).with_context(|| format!("create_dir_all {}", p.display()))?;
    }
    Ok(())
}

pub fn zip_path_to_bytes(src: &Path) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut f = File::open(src).with_context(|| format!("open {}", src.display()))?;
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn zip_directory(src_dir: &Path) -> Result<Vec<u8>> {
    let mut cursor = io::Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);

        let base = fs::canonicalize(src_dir)
            .with_context(|| format!("canonicalize {}", src_dir.display()))?;

        for entry in WalkDir::new(&base).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            let rel = path.strip_prefix(&base).unwrap_or(path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if rel_str.is_empty() || rel_str == "." {
                continue;
            }
            if path.is_dir() {
                zip.add_directory(rel_str + "/", opts)?;
            } else if path.is_file() {
                zip.start_file(rel_str, opts)?;
                let mut file = File::open(path)?;
                io::copy(&mut file, &mut zip)?;
            }
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

pub fn artifact_bytes(local_path: &Path) -> Result<Vec<u8>> {
    let meta =
        fs::metadata(local_path).with_context(|| format!("stat {}", local_path.display()))?;
    if meta.is_dir() {
        zip_directory(local_path)
    } else if local_path.extension() == Some(OsStr::new("zip")) {
        zip_path_to_bytes(local_path)
    } else {
        anyhow::bail!(
            "path must be a directory or a .zip file: {}",
            local_path.display()
        );
    }
}

pub fn run_cmd(program: &str, args: &[&str], inherit_stdio: bool) -> Result<()> {
    let mut c = Command::new(program);
    c.args(args);
    if inherit_stdio {
        c.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    }
    let st = c.status().with_context(|| format!("spawn {program}"))?;
    if !st.success() {
        anyhow::bail!("{program} {:?} failed with status {}", args, st);
    }
    Ok(())
}

pub fn output_cmd(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("spawn {program}"))?;
    if !out.status.success() {
        anyhow::bail!("{program} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn which(program: &str) -> Result<PathBuf> {
    let out = Command::new("which").arg(program).output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Ok(PathBuf::from(s))
        }
        _ => anyhow::bail!("{program} not found in PATH"),
    }
}
