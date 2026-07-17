mod config;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use config::HostEntry;
use dialoguer::{theme::ColorfulTheme, Confirm, FuzzySelect, Input};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// sshm — SSH connection manager with fuzzy-select TUI
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List all hosts
    List,
    /// Add a new host (interactive)
    Add,
    /// Edit an existing host (interactive)
    Edit {
        /// Alias to edit (fuzzy picker if omitted)
        alias: Option<String>,
    },
    /// Remove a host
    Remove {
        /// Alias to remove (fuzzy picker if omitted)
        alias: Option<String>,
    },
    /// Connect to a host (default when no subcommand)
    Connect {
        /// Alias to connect to (fuzzy picker if omitted)
        alias: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Cmd::Connect { alias: None }) => connect(None),
        Some(Cmd::Connect { alias }) => connect(alias),
        Some(Cmd::List) => list(),
        Some(Cmd::Add) => add(),
        Some(Cmd::Edit { alias }) => edit(alias),
        Some(Cmd::Remove { alias }) => remove(alias),
    }
}

fn pick(hosts: &[HostEntry], prompt: &str) -> Result<usize> {
    if hosts.is_empty() {
        bail!("no hosts in ~/.ssh/config — add one with `sshm add`");
    }
    let items: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
    let idx = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&items)
        .default(0)
        .interact()
        .context("selection cancelled")?;
    Ok(idx)
}

fn resolve<'a>(hosts: &'a [HostEntry], alias: Option<String>, prompt: &str) -> Result<&'a HostEntry> {
    match alias {
        Some(a) => hosts
            .iter()
            .find(|h| h.alias == a)
            .with_context(|| format!("host '{}' not found", a)),
        None => Ok(&hosts[pick(hosts, prompt)?]),
    }
}

fn connect(alias: Option<String>) -> Result<()> {
    let hosts = config::load_hosts()?;
    let host = resolve(&hosts, alias, "connect to")?;
    eprintln!("→ ssh {}", host.alias);
    // replace current process with ssh
    let err = Command::new("ssh").arg(&host.alias).exec();
    Err(err).context("failed to exec ssh")
}

fn list() -> Result<()> {
    let hosts = config::load_hosts()?;
    if hosts.is_empty() {
        println!("(no hosts)");
        return Ok(());
    }
    for h in hosts {
        println!("{}", h);
    }
    Ok(())
}

fn prompt_entry(theme: &ColorfulTheme, base: Option<&HostEntry>) -> Result<HostEntry> {
    let d = |s: &Option<String>| s.clone().unwrap_or_default();
    let alias: String = Input::with_theme(theme)
        .with_prompt("Alias")
        .with_initial_text(base.map(|b| b.alias.clone()).unwrap_or_default())
        .interact_text()?;
    let hostname: String = Input::with_theme(theme)
        .with_prompt("HostName (IP or domain)")
        .with_initial_text(base.map(|b| d(&b.hostname)).unwrap_or_default())
        .interact_text()?;
    let user: String = Input::with_theme(theme)
        .with_prompt("User (empty to skip)")
        .with_initial_text(base.map(|b| d(&b.user)).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;
    let port: String = Input::with_theme(theme)
        .with_prompt("Port (empty for default 22)")
        .with_initial_text(base.and_then(|b| b.port).map(|p| p.to_string()).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;
    let identity: String = Input::with_theme(theme)
        .with_prompt("IdentityFile (empty to skip)")
        .with_initial_text(base.map(|b| d(&b.identity_file)).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;

    let alias = alias.trim().to_string();
    if alias.is_empty() {
        bail!("alias is required");
    }
    if alias.chars().any(char::is_whitespace) {
        bail!("alias must not contain whitespace");
    }
    let port = match port.trim() {
        "" => None,
        p => Some(p.parse::<u16>().with_context(|| format!("invalid port '{}'", p))?),
    };

    let none_if_empty = |s: String| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) };
    Ok(HostEntry {
        alias,
        hostname: none_if_empty(hostname),
        user: none_if_empty(user),
        port,
        identity_file: none_if_empty(identity),
    })
}

fn add() -> Result<()> {
    let theme = ColorfulTheme::default();
    let entry = prompt_entry(&theme, None)?;
    config::add_host(&entry)?;
    println!("✔ added: {}", entry);
    Ok(())
}

fn edit(alias: Option<String>) -> Result<()> {
    let hosts = config::load_hosts()?;
    let target = resolve(&hosts, alias, "edit which host?")?.clone();
    let theme = ColorfulTheme::default();
    let updated = prompt_entry(&theme, Some(&target))?;
    config::update_host(&target.alias, &updated)?;
    println!("✔ updated: {}", updated);
    Ok(())
}

fn remove(alias: Option<String>) -> Result<()> {
    let hosts = config::load_hosts()?;
    let target = resolve(&hosts, alias, "remove which host?")?.clone();
    let yes = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("really remove '{}'?", target.alias))
        .default(false)
        .interact()?;
    if !yes {
        println!("cancelled");
        return Ok(());
    }
    config::remove_host(&target.alias)?;
    println!("✔ removed: {}", target.alias);
    Ok(())
}
