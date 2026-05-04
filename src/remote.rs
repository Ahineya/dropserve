use std::fs::{self, File};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Local;

use crate::util::{
    ensure_parent, expand_home, output_cmd, run_cmd, validate_domain, which, ENV_HOME,
};

const KEEP_RELEASES: usize = 5;
const SITE_CONFIG_FILE: &str = "dropserve-site.conf";

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

fn site_config_path(home: &Path, domain: &str) -> PathBuf {
    site_dir(home, domain).join(SITE_CONFIG_FILE)
}

#[derive(Debug, Clone)]
enum SiteRuntime {
    Static,
    PocketBase(PocketBaseConfig),
}

#[derive(Debug, Clone)]
struct PocketBaseConfig {
    port: u16,
    service: String,
}

fn release_stamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn validate_pb_port(port: u16) -> Result<()> {
    if port == 0 {
        anyhow::bail!("--pb-port must be between 1 and 65535");
    }
    Ok(())
}

fn pocketbase_service_name(domain: &str) -> String {
    let mut cleaned = String::with_capacity(domain.len());
    for ch in domain.chars() {
        if ch.is_ascii_alphanumeric() {
            cleaned.push(ch.to_ascii_lowercase());
        } else {
            cleaned.push('-');
        }
    }
    format!("dropserve-{cleaned}-pocketbase")
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

fn nginx_pocketbase_block(domain: &str, port: u16) -> String {
    format!(
        r#"server {{
    listen 80;
    server_name {domain};

    client_max_body_size 10M;

    location / {{
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_read_timeout 360s;

        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_pass http://127.0.0.1:{port};
    }}
}}
"#
    )
}

fn write_nginx_site_conf(home: &Path, domain: &str, runtime: &SiteRuntime) -> Result<()> {
    let nginx_dir = home.join("nginx");
    fs::create_dir_all(&nginx_dir)?;
    let path = nginx_conf_path(home, domain);
    let body = match runtime {
        SiteRuntime::Static => nginx_http_block(home, domain),
        SiteRuntime::PocketBase(cfg) => nginx_pocketbase_block(domain, cfg.port),
    };
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
    Ok(target.file_name().map(|s| s.to_string_lossy().into_owned()))
}

fn write_site_config(home: &Path, domain: &str, runtime: &SiteRuntime) -> Result<()> {
    let path = site_config_path(home, domain);
    ensure_parent(&path)?;
    let body = match runtime {
        SiteRuntime::Static => "runtime=static\n".to_string(),
        SiteRuntime::PocketBase(cfg) => {
            format!(
                "runtime=pocketbase\npb_port={}\nservice={}\n",
                cfg.port, cfg.service
            )
        }
    };
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn read_site_config(home: &Path, domain: &str) -> Result<SiteRuntime> {
    let path = site_config_path(home, domain);
    if !path.exists() {
        return Ok(SiteRuntime::Static);
    }

    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut runtime = None;
    let mut pb_port = None;
    let mut service = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "runtime" => runtime = Some(value.trim().to_string()),
            "pb_port" => pb_port = value.trim().parse::<u16>().ok(),
            "service" => service = Some(value.trim().to_string()),
            _ => {}
        }
    }

    match runtime.as_deref() {
        Some("pocketbase") => {
            let port = pb_port.ok_or_else(|| anyhow!("missing pb_port in {}", path.display()))?;
            validate_pb_port(port)?;
            let service = service.unwrap_or_else(|| pocketbase_service_name(domain));
            Ok(SiteRuntime::PocketBase(PocketBaseConfig { port, service }))
        }
        _ => Ok(SiteRuntime::Static),
    }
}

fn prepare_pocketbase_release(site: &Path, release_path: &Path) -> Result<()> {
    fs::create_dir_all(site.join("pb_data"))?;
    fs::create_dir_all(release_path.join("pb_migrations"))?;

    if release_path.join("pb_public").is_dir() {
        return Ok(());
    }

    let tmp_public = release_path.join(".dropserve-pb_public");
    if tmp_public.exists() {
        fs::remove_dir_all(&tmp_public)?;
    }
    fs::create_dir_all(&tmp_public)?;

    for entry in fs::read_dir(release_path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "pb_migrations" || name == "pb_public" || name == ".dropserve-pb_public" {
            continue;
        }
        fs::rename(entry.path(), tmp_public.join(name.as_ref()))
            .with_context(|| format!("move {} into pb_public", entry.path().display()))?;
    }

    fs::rename(&tmp_public, release_path.join("pb_public"))
        .with_context(|| "activate generated pb_public")?;
    Ok(())
}

fn pocketbase_service_path(home: &Path, service: &str) -> PathBuf {
    home.join("systemd").join(format!("{service}.service"))
}

fn write_pocketbase_service(home: &Path, domain: &str, cfg: &PocketBaseConfig) -> Result<PathBuf> {
    let pocketbase = which("pocketbase").context("pocketbase must be installed on the server")?;
    let site = site_dir(home, domain);
    let service_path = pocketbase_service_path(home, &cfg.service);
    ensure_parent(&service_path)?;

    let body = format!(
        r#"[Unit]
Description=Dropserve PocketBase {domain}
After=network.target

[Service]
Type=simple
WorkingDirectory={current}
ExecStart={pocketbase} serve --http=127.0.0.1:{port} --dir={pb_data} --migrationsDir={migrations} --publicDir={public}
Restart=always
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
"#,
        current = site.join("current").display(),
        pocketbase = pocketbase.display(),
        port = cfg.port,
        pb_data = site.join("pb_data").display(),
        migrations = site.join("current").join("pb_migrations").display(),
        public = site.join("current").join("pb_public").display(),
    );

    fs::write(&service_path, body).with_context(|| format!("write {}", service_path.display()))?;
    Ok(service_path)
}

fn systemctl(args: &[&str]) -> Result<()> {
    run_cmd("systemctl", args, true).or_else(|_| {
        let mut sudo_args = Vec::with_capacity(args.len() + 1);
        sudo_args.push("systemctl");
        sudo_args.extend_from_slice(args);
        run_cmd("sudo", &sudo_args, true)
    })
}

fn systemctl_daemon_reload() -> Result<()> {
    systemctl(&["daemon-reload"])
}

fn start_pocketbase_service(home: &Path, domain: &str, cfg: &PocketBaseConfig) -> Result<()> {
    let service_path = write_pocketbase_service(home, domain, cfg)?;
    let service_path = service_path
        .to_str()
        .ok_or_else(|| anyhow!("invalid service path"))?;
    systemctl(&["link", service_path])?;
    systemctl_daemon_reload()?;
    systemctl(&["enable", "--now", &cfg.service])?;
    systemctl(&["restart", &cfg.service])?;
    Ok(())
}

fn restart_pocketbase_service(runtime: &SiteRuntime) -> Result<()> {
    if let SiteRuntime::PocketBase(cfg) = runtime {
        systemctl(&["restart", &cfg.service])?;
    }
    Ok(())
}

fn site_type_label(runtime: &SiteRuntime) -> String {
    match runtime {
        SiteRuntime::Static => "type=static".to_string(),
        SiteRuntime::PocketBase(cfg) => {
            format!("type=backend\tbackend=pocketbase\tport={}", cfg.port)
        }
    }
}

fn remove_pocketbase_service(home: &Path, runtime: &SiteRuntime) -> Result<()> {
    if let SiteRuntime::PocketBase(cfg) = runtime {
        let _ = systemctl(&["disable", "--now", &cfg.service]);
        let service_path = pocketbase_service_path(home, &cfg.service);
        if service_path.exists() {
            fs::remove_file(&service_path)
                .with_context(|| format!("remove {}", service_path.display()))?;
        }
        let _ = systemctl_daemon_reload();
    }
    Ok(())
}

pub fn remote_create(
    home: &Path,
    domain: &str,
    zip_path: &Path,
    pocketbase: bool,
    pb_port: u16,
) -> Result<()> {
    validate_domain(domain)?;
    validate_remote_zip_path(zip_path)?;
    if pocketbase {
        validate_pb_port(pb_port)?;
    }

    let stamp = release_stamp();
    let site = site_dir(home, domain);
    let release_path = site.join("releases").join(&stamp);
    fs::create_dir_all(site.join("releases"))?;
    extract_zip(zip_path, &release_path)?;

    let runtime = if pocketbase {
        let cfg = PocketBaseConfig {
            port: pb_port,
            service: pocketbase_service_name(domain),
        };
        prepare_pocketbase_release(&site, &release_path)?;
        SiteRuntime::PocketBase(cfg)
    } else {
        SiteRuntime::Static
    };

    write_site_config(home, domain, &runtime)?;
    write_nginx_site_conf(home, domain, &runtime)?;
    set_current_atomic(&site, &stamp)?;
    if let SiteRuntime::PocketBase(cfg) = &runtime {
        start_pocketbase_service(home, domain, cfg)?;
    }

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
    let runtime = read_site_config(home, domain)?;
    if matches!(runtime, SiteRuntime::PocketBase(_)) {
        prepare_pocketbase_release(&site, &release_path)?;
    }
    set_current_atomic(&site, &stamp)?;
    restart_pocketbase_service(&runtime)?;

    prune_releases(&site)?;
    let _ = fs::remove_file(zip_path);
    Ok(())
}

pub fn remote_delete(home: &Path, domain: &str) -> Result<()> {
    validate_domain(domain)?;
    let site = site_dir(home, domain);
    let runtime = read_site_config(home, domain)?;
    remove_pocketbase_service(home, &runtime)?;
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
        let site_type = site_type_label(&read_site_config(home, &d)?);
        println!("{d}\tcurrent={cur}\t{site_type}");
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
    let runtime = read_site_config(home, domain)?;
    restart_pocketbase_service(&runtime)?;
    nginx_reload()?;
    Ok(())
}

fn nginx_reload() -> Result<()> {
    let nginx = which("nginx").unwrap_or_else(|_| PathBuf::from("/usr/sbin/nginx"));
    run_cmd(nginx.to_str().unwrap_or("nginx"), &["-s", "reload"], true)
        .or_else(|_| run_cmd("sudo", &["nginx", "-s", "reload"], true))
}

fn public_ipv4() -> Result<String> {
    output_cmd(
        "curl",
        &["-sSf", "--connect-timeout", "5", "https://api.ipify.org"],
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
            println!("Still waiting... ({}/{})", attempt + 1, max_attempts);
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
        &[
            "--nginx",
            "-d",
            domain,
            "--non-interactive",
            "--agree-tos",
            "--register-unsafely-without-email",
        ],
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
    fs::create_dir_all(home.join("systemd"))?;
    let hook = home.join("dropserve.conf");
    let nginx_glob = home.join("nginx").join("*.conf").display().to_string();
    let body = format!("include {nginx_glob};\n");
    ensure_parent(&hook)?;
    fs::write(&hook, body).with_context(|| format!("write {}", hook.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dropserve-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn pocketbase_service_name_is_systemd_safe() {
        assert_eq!(
            pocketbase_service_name("App.Example.COM"),
            "dropserve-app-example-com-pocketbase"
        );
    }

    #[test]
    fn pocketbase_nginx_proxies_to_local_port() {
        let block = nginx_pocketbase_block("app.example.com", 8123);
        assert!(block.contains("server_name app.example.com;"));
        assert!(block.contains("proxy_pass http://127.0.0.1:8123;"));
        assert!(!block.contains("root "));
    }

    #[test]
    fn site_config_round_trips_pocketbase_runtime() {
        let home = test_dir("site-config");
        let runtime = SiteRuntime::PocketBase(PocketBaseConfig {
            port: 8099,
            service: "dropserve-app-example-com-pocketbase".to_string(),
        });

        write_site_config(&home, "app.example.com", &runtime).unwrap();
        let parsed = read_site_config(&home, "app.example.com").unwrap();

        match parsed {
            SiteRuntime::PocketBase(cfg) => {
                assert_eq!(cfg.port, 8099);
                assert_eq!(cfg.service, "dropserve-app-example-com-pocketbase");
            }
            SiteRuntime::Static => panic!("expected pocketbase runtime"),
        }

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn pocketbase_release_wraps_plain_artifact_as_pb_public() {
        let home = test_dir("pb-public");
        let site = home.join("sites").join("app.example.com");
        let release = site.join("releases").join("20260504-100000");
        fs::create_dir_all(release.join("assets")).unwrap();
        fs::write(release.join("index.html"), "<h1>app</h1>").unwrap();
        fs::write(release.join("assets").join("app.js"), "console.log(1)").unwrap();

        prepare_pocketbase_release(&site, &release).unwrap();

        assert!(site.join("pb_data").is_dir());
        assert!(release.join("pb_migrations").is_dir());
        assert!(release.join("pb_public").join("index.html").is_file());
        assert!(release
            .join("pb_public")
            .join("assets")
            .join("app.js")
            .is_file());
        assert!(!release.join("index.html").exists());

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn list_type_labels_are_clear() {
        let static_label = site_type_label(&SiteRuntime::Static);
        let backend_label = site_type_label(&SiteRuntime::PocketBase(PocketBaseConfig {
            port: 8090,
            service: "dropserve-app-example-com-pocketbase".to_string(),
        }));

        assert_eq!(static_label, "type=static");
        assert_eq!(backend_label, "type=backend\tbackend=pocketbase\tport=8090");
    }
}
