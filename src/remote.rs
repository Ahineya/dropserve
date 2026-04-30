use std::fs::{self, File};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::os::unix::fs::symlink;

use anyhow::{anyhow, Context, Result};
use chrono::Local;

use crate::util::{
    ensure_parent, expand_home, output_cmd, run_cmd, validate_domain, which, ENV_HOME,
};

const KEEP_RELEASES: usize = 5;

/// Written by `dropserve serve` so `dropserve remote` over SSH (non-login shell, no env) uses the same tree nginx includes.
pub const ETC_DROPSERVE_HOME_FILE: &str = "/etc/dropserve/home";

pub fn persist_server_home_for_remote(home: &Path) -> Result<()> {
    let etc = Path::new("/etc/dropserve");
    fs::create_dir_all(etc).with_context(|| format!("create {}", etc.display()))?;
    let canon = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let path = etc.join("home");
    fs::write(&path, format!("{}\n", canon.display()))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn resolve_home(main_dir: Option<PathBuf>) -> PathBuf {
    if let Some(p) = main_dir {
        return expand_home(&p);
    }
    if let Some(raw) = std::env::var_os(ENV_HOME) {
        let s = raw.to_string_lossy();
        let t = s.trim();
        if !t.is_empty() {
            return expand_home(Path::new(t));
        }
    }
    if let Ok(text) = fs::read_to_string(ETC_DROPSERVE_HOME_FILE) {
        let t = text.trim();
        if !t.is_empty() {
            let p = PathBuf::from(t);
            if p.is_absolute() {
                return p;
            }
        }
    }
    crate::util::default_home()
}

fn site_dir(home: &Path, domain: &str) -> PathBuf {
    home.join("sites").join(domain)
}

fn nginx_conf_path(home: &Path, domain: &str) -> PathBuf {
    home.join("nginx").join(format!("{domain}.conf"))
}

fn release_stamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S").to_string()
}

pub fn validate_remote_zip_path(zip_path: &Path) -> Result<()> {
    let name = zip_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid zip path"))?;
    if !name.starts_with("dropserve-upload-") || !name.ends_with(".zip") {
        anyhow::bail!("refusing to use artifact outside dropserve upload naming");
    }
    let canon = fs::canonicalize(zip_path).with_context(|| zip_path.display().to_string())?;
    let parent = canon
        .parent()
        .ok_or_else(|| anyhow!("artifact has no parent directory"))?;
    let tmp = fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/tmp"));
    if parent != tmp.as_path() {
        anyhow::bail!("artifact must live under /tmp");
    }
    Ok(())
}

fn nginx_http_block(home: &Path, domain: &str) -> String {
    let root = home
        .join("sites")
        .join(domain)
        .join("current")
        .display()
        .to_string();
    format!(
        r#"server {{
    listen 80;
    server_name {domain};

    root {root};
    index index.html;

    location / {{
        try_files $uri $uri/ /index.html;
    }}
}}
"#
    )
}

fn write_nginx_site_conf(home: &Path, domain: &str) -> Result<()> {
    let nginx_dir = home.join("nginx");
    fs::create_dir_all(&nginx_dir)?;
    let path = nginx_conf_path(home, domain);
    let body = nginx_http_block(home, domain);
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn remove_nginx_site_conf(home: &Path, domain: &str) -> Result<()> {
    let path = nginx_conf_path(home, domain);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_current_atomic(site_root: &Path, release_name: &str) -> Result<()> {
    let rel_target = PathBuf::from("releases").join(release_name);
    let current = site_root.join("current");
    let tmp = site_root.join(".current_new");
    if tmp.exists() {
        fs::remove_file(&tmp)?;
    }
    symlink(&rel_target, &tmp).with_context(|| "symlink temp current")?;
    fs::rename(&tmp, &current).with_context(|| "rename current symlink")?;
    Ok(())
}

#[cfg(not(unix))]
fn set_current_atomic(_site_root: &Path, _release_name: &str) -> Result<()> {
    anyhow::bail!("dropserve remote operations require Unix");
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut inner = archive.by_index(i)?;
        let outpath = match inner.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue,
        };
        if inner.name().ends_with('/') {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                fs::create_dir_all(p)?;
            }
            let mut outfile = File::create(&outpath)?;
            std::io::copy(&mut inner, &mut outfile)?;
        }
    }
    Ok(())
}

fn list_release_dirs(site_root: &Path) -> Result<Vec<String>> {
    let releases = site_root.join("releases");
    if !releases.is_dir() {
        return Ok(vec![]);
    }
    let mut names: Vec<String> = fs::read_dir(&releases)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

fn prune_releases(site_root: &Path) -> Result<()> {
    let mut names = list_release_dirs(site_root)?;
    if names.len() <= KEEP_RELEASES {
        return Ok(());
    }
    let drop = names.len() - KEEP_RELEASES;
    for n in names.drain(..drop) {
        let p = site_root.join("releases").join(&n);
        fs::remove_dir_all(&p).with_context(|| format!("prune {}", p.display()))?;
    }
    Ok(())
}

fn read_current_release(site_root: &Path) -> Result<Option<String>> {
    let cur = site_root.join("current");
    let is_link = match fs::symlink_metadata(&cur) {
        Ok(m) => m.file_type().is_symlink(),
        Err(_) => false,
    };
    if !is_link {
        return Ok(None);
    }
    let target = fs::read_link(&cur)?;
    Ok(target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned()))
}

pub fn remote_create(home: &Path, domain: &str, zip_path: &Path) -> Result<()> {
    validate_domain(domain)?;
    validate_remote_zip_path(zip_path)?;

    let stamp = release_stamp();
    let site = site_dir(home, domain);
    let release_path = site.join("releases").join(&stamp);
    fs::create_dir_all(site.join("releases"))?;
    extract_zip(zip_path, &release_path)?;

    write_nginx_site_conf(home, domain)?;
    set_current_atomic(&site, &stamp)?;

    nginx_reload()?;

    wait_for_dns_then_certbot(domain)?;
    nginx_reload()?;

    prune_releases(&site)?;
    let _ = fs::remove_file(zip_path);
    Ok(())
}

pub fn remote_update(home: &Path, domain: &str, zip_path: &Path) -> Result<()> {
    validate_domain(domain)?;
    validate_remote_zip_path(zip_path)?;

    let site = site_dir(home, domain);
    if !site.join("releases").is_dir() {
        anyhow::bail!("site {} has no releases; run create first", domain);
    }

    let stamp = release_stamp();
    let release_path = site.join("releases").join(&stamp);
    extract_zip(zip_path, &release_path)?;
    set_current_atomic(&site, &stamp)?;

    prune_releases(&site)?;
    let _ = fs::remove_file(zip_path);
    Ok(())
}

pub fn remote_delete(home: &Path, domain: &str) -> Result<()> {
    validate_domain(domain)?;
    let site = site_dir(home, domain);
    remove_nginx_site_conf(home, domain)?;
    if site.exists() {
        fs::remove_dir_all(&site).with_context(|| format!("remove {}", site.display()))?;
    }
    nginx_reload()?;
    Ok(())
}

pub fn remote_list(home: &Path) -> Result<()> {
    let base = home.join("sites");
    if !base.is_dir() {
        println!("(no sites)");
        return Ok(());
    }
    let mut domains: Vec<_> = fs::read_dir(&base)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    domains.sort();
    for d in domains {
        let site = site_dir(home, &d);
        let cur = read_current_release(&site)?.unwrap_or_else(|| "?".into());
        println!("{d}\tcurrent={cur}");
    }
    Ok(())
}

pub fn remote_rollback(home: &Path, domain: &str) -> Result<()> {
    validate_domain(domain)?;
    let site = site_dir(home, domain);
    let names = list_release_dirs(&site)?;
    let cur = read_current_release(&site)?
        .ok_or_else(|| anyhow!("site {} has no current release", domain))?;
    let pos = names
        .iter()
        .position(|n| n == &cur)
        .ok_or_else(|| anyhow!("current release not found in releases/"))?;
    if pos == 0 {
        anyhow::bail!("no previous release to roll back to");
    }
    let prev = names[pos - 1].clone();
    set_current_atomic(&site, &prev)?;
    nginx_reload()?;
    Ok(())
}

fn nginx_reload() -> Result<()> {
    let nginx = which("nginx").unwrap_or_else(|_| PathBuf::from("/usr/sbin/nginx"));
    run_cmd(
        nginx.to_str().unwrap_or("nginx"),
        &["-s", "reload"],
        true,
    )
    .or_else(|_| run_cmd("sudo", &["nginx", "-s", "reload"], true))
}

fn public_ipv4() -> Result<String> {
    output_cmd(
        "curl",
        &[
            "-sSf",
            "--connect-timeout",
            "5",
            "https://api.ipify.org",
        ],
    )
    .or_else(|_| output_cmd("curl", &["-sSf", "https://ifconfig.me/ip"]))
}

fn dig_a(domain: &str) -> Result<Vec<String>> {
    let out = output_cmd("dig", &["+short", domain, "A"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

fn wait_for_dns_then_certbot(domain: &str) -> Result<()> {
    let server_ip = public_ipv4().context("could not determine server public IP (curl)")?;
    println!("Waiting for DNS A record for {domain} to match server IP {server_ip}...");

    let max_attempts = 360;
    for attempt in 0..max_attempts {
        if let Ok(records) = dig_a(domain) {
            if records.iter().any(|r| r == &server_ip) {
                println!("DNS matches. Running certbot...");
                run_certbot(domain)?;
                return Ok(());
            }
        }
        if attempt == 0 || attempt % 12 == 0 {
            println!(
                "Still waiting... ({}/{})",
                attempt + 1,
                max_attempts
            );
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    anyhow::bail!(
        "timed out waiting for DNS; point {domain} A record to {server_ip} and run certbot manually"
    )
}

fn run_certbot(domain: &str) -> Result<()> {
    let certbot = which("certbot")?;
    run_cmd(
        certbot.to_str().unwrap_or("certbot"),
        &["--nginx", "-d", domain, "--non-interactive", "--agree-tos", "--register-unsafely-without-email"],
        true,
    )
    .or_else(|_| {
        run_cmd(
            "sudo",
            &[
                "certbot",
                "--nginx",
                "-d",
                domain,
                "--non-interactive",
                "--agree-tos",
                "--register-unsafely-without-email",
            ],
            true,
        )
    })
}

pub fn write_stub_home(home: &Path) -> Result<()> {
    fs::create_dir_all(home.join("sites"))?;
    fs::create_dir_all(home.join("nginx"))?;
    let hook = home.join("dropserve.conf");
    let nginx_glob = home.join("nginx").join("*.conf").display().to_string();
    let body = format!("include {nginx_glob};\n");
    ensure_parent(&hook)?;
    fs::write(&hook, body).with_context(|| format!("write {}", hook.display()))?;
    Ok(())
}
