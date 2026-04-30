use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use regex::Regex;

use crate::remote::{persist_server_home_for_remote, resolve_home, write_stub_home};
use crate::util::{run_cmd, which};

const NGINX_MARKER: &str = "# include added by dropserve";

pub fn run(main_dir: Option<PathBuf>, nginx_conf: Option<PathBuf>) -> Result<()> {
    let custom_home = main_dir.is_some();
    let home = resolve_home(main_dir);
    let nginx_conf = nginx_conf.unwrap_or_else(|| PathBuf::from("/etc/nginx/nginx.conf"));

    which("certbot").context("certbot must be installed on the server")?;
    which("nginx").context("nginx must be installed on the server")?;

    write_stub_home(&home)?;
    persist_server_home_for_remote(&home)?;
    ensure_nginx_include(&nginx_conf, &home)?;

    let nginx = which("nginx").unwrap_or_else(|_| PathBuf::from("/usr/sbin/nginx"));
    run_cmd(
        nginx.to_str().unwrap_or("nginx"),
        &["-t"],
        true,
    )
    .or_else(|_| run_cmd("sudo", &[nginx.to_str().unwrap_or("nginx"), "-t"], true))
    .context("nginx -t failed after updating configuration")?;

    let home_display = fs::canonicalize(&home).unwrap_or_else(|_| home.clone());
    if home_display.starts_with(Path::new("/root")) {
        eprintln!(
            "warning: dropserve home is under /root. nginx workers (typically www-data) must be able to \
             traverse every directory in the path to reach sites/. If you see 403/500 in the browser, \
             move DROPSERVE_HOME to e.g. /var/lib/dropserve or grant careful execute permission on /root; \
             check /var/log/nginx/error.log for Permission denied."
        );
    }
    println!(
        "dropserve server layout ready at {}",
        home_display.display()
    );
    println!(
        "Ensure nginx loads {} (dropserve serve adds this if missing).",
        nginx_conf.display()
    );
    println!(
        "`dropserve remote` resolves the server tree as: $DROPSERVE_HOME, else {}, else ~/.dropserve.",
        crate::remote::ETC_DROPSERVE_HOME_FILE
    );
    if custom_home {
        println!(
            "Override for one-off commands: DROPSERVE_HOME={}",
            home_display.display()
        );
    }
    Ok(())
}

fn ensure_nginx_include(nginx_conf: &Path, home: &Path) -> Result<()> {
    let hook = home.join("dropserve.conf");
    let hook_abs = fs::canonicalize(&hook).unwrap_or_else(|_| hook.clone());
    let include_line = format!("include {};", hook_abs.display());

    if !nginx_conf.is_file() {
        anyhow::bail!("nginx config not found: {}", nginx_conf.display());
    }

    let raw = fs::read_to_string(nginx_conf)
        .with_context(|| format!("read {}", nginx_conf.display()))?;

    let content = strip_prior_dropserve_http_include(&raw);
    let stripped_trailing_junk = content != raw;

    let http_close = find_http_block_closing_brace(&content)
        .ok_or_else(|| anyhow!("no `http {{ ... }}` block found in {}", nginx_conf.display()))?;

    if dropserve_include_present_inside_http(&content, &include_line, http_close) {
        if stripped_trailing_junk {
            fs::write(nginx_conf, &content)
                .with_context(|| format!("write {}", nginx_conf.display()))?;
        }
        return Ok(());
    }

    let insert = format!("\n\t{NGINX_MARKER}\n\t{include_line}\n");
    let new_content = format!(
        "{}{}{}",
        &content[..http_close],
        insert,
        &content[http_close..]
    );

    fs::write(nginx_conf, new_content)
        .with_context(|| format!("write {}", nginx_conf.display()))?;
    Ok(())
}

fn strip_prior_dropserve_http_include(content: &str) -> String {
    let re = Regex::new(
        r"(?m)^\s*# include added by dropserve\s*\r?\n\s*include\s+[^\r\n]*dropserve\.conf\s*;\s*\r?\n",
    )
    .expect("valid regex");
    re.replace_all(content, "").to_string()
}

fn find_http_open_brace(content: &str) -> Option<usize> {
    let mut offset = 0usize;
    while offset < content.len() {
        let rest = &content[offset..];
        let line_end_rel = rest.find('\n');
        let (line, advance) = match line_end_rel {
            Some(i) => (&rest[..i], i + 1),
            None => (rest, rest.len()),
        };
        let line = line.trim_end_matches('\r');
        let line_start = offset;
        offset += advance;
        if let Some(b) = http_line_open_brace_byte(line, line_start) {
            return Some(b);
        }
    }
    None
}

fn http_line_open_brace_byte(line: &str, line_byte_start: usize) -> Option<usize> {
    let t = line.trim_start();
    if !t.starts_with("http") || t.starts_with("https") {
        return None;
    }
    let after = t.get(4..)?.trim_start();
    if !after.starts_with('{') {
        return None;
    }
    let brace_rel = line.find('{')?;
    Some(line_byte_start + brace_rel)
}

fn matching_brace_close(content: &str, open_brace_idx: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    if bytes.get(open_brace_idx) != Some(&b'{') {
        return None;
    }
    let mut i = open_brace_idx + 1;
    let mut depth = 1u32;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = (i + 2).min(bytes.len()),
                        b'\'' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = (i + 2).min(bytes.len()),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn find_http_block_closing_brace(content: &str) -> Option<usize> {
    let open_brace = find_http_open_brace(content)?;
    matching_brace_close(content, open_brace)
}

fn dropserve_include_present_inside_http(content: &str, include_line: &str, http_close: usize) -> bool {
    content
        .find(include_line)
        .is_some_and(|pos| pos < http_close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_before_http_close() {
        let conf = r#"user www-data;
events { worker_connections 768; }
http {
	include /etc/nginx/conf.d/*.conf;
	include /etc/nginx/sites-enabled/*;

}
"#;
        let close = find_http_block_closing_brace(conf).unwrap();
        assert!(conf.as_bytes()[close] == b'}');
        let insert_pos = close;
        let include_line = "include /tmp/.dropserve/dropserve.conf;";
        assert!(!dropserve_include_present_inside_http(conf, include_line, insert_pos));
        let insert = format!("\n\t{NGINX_MARKER}\n\t{include_line}\n");
        let new_conf = format!(
            "{}{}{}",
            &conf[..insert_pos],
            insert,
            &conf[insert_pos..]
        );
        assert!(new_conf.contains("http {\n"));
        assert!(new_conf.contains(NGINX_MARKER));
        assert!(new_conf.lines().any(|l| l.contains("sites-enabled")));
        let cb = find_http_block_closing_brace(&new_conf).unwrap();
        assert!(new_conf[..cb].contains(include_line));
    }

    #[test]
    fn strips_previous_stanza() {
        let conf = "http {\n}\n# include added by dropserve\ninclude /root/.dropserve/dropserve.conf;\n";
        let cleaned = strip_prior_dropserve_http_include(conf);
        assert!(!cleaned.contains("dropserve"));
        assert!(cleaned.contains("http"));
    }
}
