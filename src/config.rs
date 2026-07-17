use anyhow::{bail, Context, Result};
use std::fmt;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct HostEntry {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

impl fmt::Display for HostEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let user = self.user.as_deref().unwrap_or("-");
        let host = self.hostname.as_deref().unwrap_or("-");
        let port = self.port.map(|p| p.to_string()).unwrap_or_else(|| "22".into());
        write!(f, "{:<20} {}@{}:{}", self.alias, user, host, port)
    }
}

pub fn ssh_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("home directory not found")?;
    Ok(home.join(".ssh").join("config"))
}

/// Parse ~/.ssh/config into host entries. Wildcard hosts (*, ?) are skipped.
pub fn load_hosts() -> Result<Vec<HostEntry>> {
    let path = ssh_config_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(parse(&content))
}

pub fn parse(content: &str) -> Vec<HostEntry> {
    let mut hosts: Vec<HostEntry> = Vec::new();
    let mut current: Vec<HostEntry> = Vec::new();

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, |c: char| c.is_whitespace() || c == '=');
        let key = parts.next().unwrap_or("").to_ascii_lowercase();
        let value = parts.next().unwrap_or("").trim().trim_matches('"');

        match key.as_str() {
            "host" => {
                hosts.append(&mut current);
                for alias in value.split_whitespace() {
                    if alias.contains('*') || alias.contains('?') || alias.starts_with('!') {
                        continue;
                    }
                    current.push(HostEntry { alias: alias.to_string(), ..Default::default() });
                }
            }
            "hostname" => current.iter_mut().for_each(|h| h.hostname = Some(value.to_string())),
            "user" => current.iter_mut().for_each(|h| h.user = Some(value.to_string())),
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    current.iter_mut().for_each(|h| h.port = Some(p));
                }
            }
            "identityfile" => {
                current.iter_mut().for_each(|h| h.identity_file = Some(value.to_string()))
            }
            _ => {}
        }
    }
    hosts.append(&mut current);
    hosts
}

fn render_block(entry: &HostEntry) -> String {
    let mut s = format!("Host {}\n", entry.alias);
    if let Some(h) = &entry.hostname {
        s += &format!("    HostName {}\n", h);
    }
    if let Some(u) = &entry.user {
        s += &format!("    User {}\n", u);
    }
    if let Some(p) = entry.port {
        if p != 22 {
            s += &format!("    Port {}\n", p);
        }
    }
    if let Some(i) = &entry.identity_file {
        s += &format!("    IdentityFile {}\n", i);
    }
    s
}

fn backup(path: &PathBuf) -> Result<()> {
    if path.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
        let bak = path.with_extension(format!("bak.{}", stamp));
        fs::copy(path, &bak)?;
    }
    Ok(())
}

/// Append a new Host block to ~/.ssh/config (with timestamped backup).
pub fn add_host(entry: &HostEntry) -> Result<()> {
    let path = ssh_config_path()?;
    if load_hosts()?.iter().any(|h| h.alias == entry.alias) {
        bail!("host '{}' already exists (use `sshm edit`)", entry.alias);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    backup(&path)?;
    let existing = if path.exists() { fs::read_to_string(&path)? } else { String::new() };
    let sep = if existing.is_empty() || existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    fs::write(&path, format!("{}{}{}", existing, sep, render_block(entry)))?;
    Ok(())
}

/// Replace an existing Host block. Rewrites the file, preserving all other
/// lines verbatim; only lines belonging to the target block are replaced.
pub fn update_host(old_alias: &str, entry: &HostEntry) -> Result<()> {
    let path = ssh_config_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut out: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut found = false;

    for raw in content.lines() {
        let trimmed = raw.trim();
        let is_host_line = trimmed.to_ascii_lowercase().starts_with("host ")
            || trimmed.to_ascii_lowercase() == "host";
        if is_host_line {
            let aliases: Vec<&str> = trimmed.splitn(2, char::is_whitespace)
                .nth(1).unwrap_or("").split_whitespace().collect();
            if aliases == [old_alias] {
                // entering the block we want to replace
                in_target = true;
                found = true;
                out.push(render_block(entry).trim_end().to_string());
                continue;
            }
            in_target = false;
        }
        if !in_target {
            out.push(raw.to_string());
        }
    }

    if !found {
        bail!(
            "host '{}' not found as a standalone block; edit ~/.ssh/config manually \
             (it may share a Host line with other aliases)",
            old_alias
        );
    }

    backup(&path)?;
    fs::write(&path, out.join("\n") + "\n")?;
    Ok(())
}

/// Remove a Host block entirely.
pub fn remove_host(alias: &str) -> Result<()> {
    let path = ssh_config_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut out: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut found = false;

    for raw in content.lines() {
        let trimmed = raw.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("host ") || lower == "host" {
            let aliases: Vec<&str> = trimmed.splitn(2, char::is_whitespace)
                .nth(1).unwrap_or("").split_whitespace().collect();
            if aliases == [alias] {
                in_target = true;
                found = true;
                continue;
            }
            in_target = false;
        }
        if !in_target {
            out.push(raw.to_string());
        }
    }

    if !found {
        bail!(
            "host '{}' not found as a standalone block; edit ~/.ssh/config manually",
            alias
        );
    }

    backup(&path)?;
    fs::write(&path, out.join("\n") + "\n")?;
    Ok(())
}
