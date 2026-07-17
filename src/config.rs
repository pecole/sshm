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

/// Split a config line into its lowercased keyword and the rest of the line.
/// ssh_config separates them with whitespace or an optional '=' surrounded by
/// optional whitespace. Returns None for blank and comment lines.
fn split_keyword(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let end = trimmed
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(trimmed.len());
    let key = trimmed[..end].to_ascii_lowercase();
    let mut rest = trimmed[end..].trim_start();
    if let Some(stripped) = rest.strip_prefix('=') {
        rest = stripped.trim_start();
    }
    Some((key, rest))
}

pub fn parse(content: &str) -> Vec<HostEntry> {
    let mut hosts: Vec<HostEntry> = Vec::new();
    let mut current: Vec<HostEntry> = Vec::new();

    for raw in content.lines() {
        let Some((key, value)) = split_keyword(raw) else { continue };
        let value = value.trim_end().trim_matches('"');

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
            // options under a Match line belong to the match scope, not the
            // preceding Host block
            "match" => hosts.append(&mut current),
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
        s += &format!("    Port {}\n", p);
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

/// Locate the block whose Host line declares exactly `[alias]`.
/// Returns (start, end) line indices; end is exclusive and excludes trailing
/// blank and comment lines, which conventionally belong to the next block.
fn find_block(lines: &[&str], alias: &str) -> Option<(usize, usize)> {
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let Some((key, rest)) = split_keyword(line) else { continue };
        if key != "host" && key != "match" {
            continue;
        }
        if let Some(s) = start {
            return Some(trim_block(lines, s, i));
        }
        if key == "host" && rest.split_whitespace().collect::<Vec<_>>() == [alias] {
            start = Some(i);
        }
    }
    start.map(|s| trim_block(lines, s, lines.len()))
}

fn trim_block(lines: &[&str], start: usize, mut end: usize) -> (usize, usize) {
    while end > start + 1 {
        let t = lines[end - 1].trim();
        if t.is_empty() || t.starts_with('#') {
            end -= 1;
        } else {
            break;
        }
    }
    (start, end)
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Replace an existing Host block in place. Only the lines sshm models
/// (Host, HostName, User, Port, IdentityFile) are rewritten; every other
/// line in the block — and the rest of the file — is preserved verbatim.
pub fn update_host(old_alias: &str, entry: &HostEntry) -> Result<()> {
    if entry.alias != old_alias && load_hosts()?.iter().any(|h| h.alias == entry.alias) {
        bail!("host '{}' already exists", entry.alias);
    }
    let path = ssh_config_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let updated = update_in(&content, old_alias, entry)?;
    backup(&path)?;
    fs::write(&path, updated)?;
    Ok(())
}

fn update_in(content: &str, old_alias: &str, entry: &HostEntry) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let Some((start, end)) = find_block(&lines, old_alias) else {
        bail!(
            "host '{}' not found as a standalone block; edit ~/.ssh/config manually \
             (it may share a Host line with other aliases)",
            old_alias
        );
    };

    let mut block: Vec<String> = vec![format!("{}Host {}", indent_of(lines[start]), entry.alias)];
    let mut pending = [
        ("HostName", entry.hostname.clone()),
        ("User", entry.user.clone()),
        ("Port", entry.port.map(|p| p.to_string())),
        ("IdentityFile", entry.identity_file.clone()),
    ];
    let mut opt_indent: Option<String> = None;
    for &line in &lines[start + 1..end] {
        let Some((key, _)) = split_keyword(line) else {
            block.push(line.to_string());
            continue;
        };
        if opt_indent.is_none() {
            opt_indent = Some(indent_of(line).to_string());
        }
        match pending.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            // first occurrence gets the new value; duplicates and cleared
            // values drop the line
            Some((k, v)) => {
                if let Some(v) = v.take() {
                    block.push(format!("{}{} {}", indent_of(line), k, v));
                }
            }
            None => block.push(line.to_string()),
        }
    }
    let indent = opt_indent.as_deref().unwrap_or("    ").to_string();
    for (k, v) in pending {
        if let Some(v) = v {
            block.push(format!("{}{} {}", indent, k, v));
        }
    }

    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.append(&mut block);
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    Ok(out.join("\n") + "\n")
}

/// Remove a Host block entirely, leaving surrounding blocks, Match blocks,
/// and comments intact.
pub fn remove_host(alias: &str) -> Result<()> {
    let path = ssh_config_path()?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let updated = remove_in(&content, alias)?;
    backup(&path)?;
    fs::write(&path, updated)?;
    Ok(())
}

fn remove_in(content: &str, alias: &str) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let Some((start, mut end)) = find_block(&lines, alias) else {
        bail!(
            "host '{}' not found as a standalone block; edit ~/.ssh/config manually",
            alias
        );
    };
    // swallow the blank separator the removed block leaves behind
    while end < lines.len() && lines[end].trim().is_empty() {
        end += 1;
    }
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        return Ok(String::new());
    }
    Ok(out.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(alias: &str) -> HostEntry {
        HostEntry { alias: alias.to_string(), ..Default::default() }
    }

    #[test]
    fn parse_key_equals_value() {
        let hosts = parse("Host a\n    HostName = example.com\n    Port=2222\n");
        assert_eq!(hosts[0].hostname.as_deref(), Some("example.com"));
        assert_eq!(hosts[0].port, Some(2222));
    }

    #[test]
    fn parse_does_not_apply_match_options_to_previous_host() {
        let hosts = parse("Host web\n    HostName web.example.com\nMatch host *.internal\n    User root\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].user, None);
    }

    #[test]
    fn update_preserves_unmodeled_options() {
        let input = "Host prod\n    HostName x\n    ProxyJump bastion\n";
        let e = HostEntry {
            alias: "prod".into(),
            hostname: Some("x".into()),
            user: Some("deploy".into()),
            ..Default::default()
        };
        let out = update_in(input, "prod", &e).unwrap();
        assert!(out.contains("ProxyJump bastion"));
        assert!(out.contains("User deploy"));
        assert!(out.contains("HostName x"));
    }

    #[test]
    fn update_clears_removed_option() {
        let input = "Host a\n    HostName x\n    User root\n";
        let e = HostEntry { alias: "a".into(), hostname: Some("x".into()), ..Default::default() };
        let out = update_in(input, "a", &e).unwrap();
        assert!(!out.contains("User root"));
    }

    #[test]
    fn update_keeps_explicit_port_22() {
        let input = "Host legacy\n    HostName x\n    Port 22\n\nHost *\n    Port 2222\n";
        let e = HostEntry {
            alias: "legacy".into(),
            hostname: Some("x".into()),
            port: Some(22),
            ..Default::default()
        };
        let out = update_in(input, "legacy", &e).unwrap();
        assert!(out.contains("Port 22\n"));
        assert!(out.contains("Host *\n    Port 2222"));
    }

    #[test]
    fn update_stops_at_match_block() {
        let input = "Host a\n    HostName x\nMatch host y\n    ProxyJump z\n";
        let e = HostEntry { alias: "a".into(), hostname: Some("x2".into()), ..Default::default() };
        let out = update_in(input, "a", &e).unwrap();
        assert!(out.contains("Match host y\n    ProxyJump z"));
        assert!(out.contains("HostName x2"));
    }

    #[test]
    fn update_stops_at_tab_separated_host() {
        let input = "Host a\n    HostName x\nHost\tb\n    HostName y\n";
        let e = HostEntry { alias: "a".into(), hostname: Some("x2".into()), ..Default::default() };
        let out = update_in(input, "a", &e).unwrap();
        assert!(out.contains("Host\tb\n    HostName y"));
    }

    #[test]
    fn update_preserves_comments_and_blanks_before_next_host() {
        let input = "Host foo\n    HostName x\n\n# production DB — do not remove\nHost db\n    HostName d\n";
        let e = HostEntry { alias: "foo".into(), hostname: Some("x2".into()), ..Default::default() };
        let out = update_in(input, "foo", &e).unwrap();
        assert!(out.contains("# production DB — do not remove\nHost db"));
    }

    #[test]
    fn update_preserves_interior_comments() {
        let input = "Host a\n    HostName x\n    # keep me\n    ProxyJump j\n";
        let e = HostEntry { alias: "a".into(), hostname: Some("x".into()), ..Default::default() };
        let out = update_in(input, "a", &e).unwrap();
        assert!(out.contains("    # keep me\n    ProxyJump j"));
    }

    #[test]
    fn update_missing_host_errors() {
        assert!(update_in("Host a\n    HostName x\n", "b", &entry("b")).is_err());
    }

    #[test]
    fn update_multi_alias_host_errors() {
        assert!(update_in("Host a b\n    HostName x\n", "a", &entry("a")).is_err());
    }

    #[test]
    fn remove_stops_at_match_and_tab_host() {
        let input = "Host a\n    HostName x\nMatch host y\n    ProxyJump z\nHost\tb\n    HostName yb\n";
        let out = remove_in(input, "a").unwrap();
        assert!(!out.contains("HostName x"));
        assert!(out.contains("Match host y\n    ProxyJump z"));
        assert!(out.contains("Host\tb\n    HostName yb"));
    }

    #[test]
    fn remove_preserves_comment_before_next_host() {
        let input = "Host foo\n    HostName x\n\n# note about db\nHost db\n    HostName d\n";
        let out = remove_in(input, "foo").unwrap();
        assert_eq!(out, "# note about db\nHost db\n    HostName d\n");
    }

    #[test]
    fn remove_last_block_leaves_rest() {
        let input = "Host a\n    HostName x\n\nHost b\n    HostName y\n";
        let out = remove_in(input, "b").unwrap();
        assert_eq!(out, "Host a\n    HostName x\n");
    }
}
