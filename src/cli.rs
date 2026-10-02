//! Client subcommands (`raahi apply`, `raahi dump`) that talk to a running
//! Raahi's admin API. Config files are parsed here, not on the server, so
//! `${VAR}` references are filled from this machine's environment and secrets
//! can stay out of the file.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use clap::{Args, Subcommand};
use raahi_core::{ConfigFormat, parse_config};
use serde_json::Value;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Apply a config file (YAML, HUML or JSON) to a running Raahi.
    Apply {
        /// Config file, or `-` for stdin.
        #[arg(short, long)]
        file: PathBuf,
        /// File format: yaml, huml or json (default: from the file extension).
        #[arg(long)]
        format: Option<String>,
        /// Print what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        remote: Remote,
    },
    /// Print the running config as a config file.
    Dump {
        /// Output format: yaml, huml or json.
        #[arg(long, default_value = "yaml")]
        format: String,
        #[command(flatten)]
        remote: Remote,
    },
}

#[derive(Args, Debug)]
pub struct Remote {
    /// Admin API base URL.
    #[arg(long, env = "RAAHI_URL", default_value = "http://127.0.0.1:9080")]
    url: String,
    /// Admin token, sent as a bearer token.
    #[arg(long, env = "RAAHI_TOKEN", hide_env_values = true)]
    token: Option<String>,
}

impl Remote {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}/api/v1{path}", self.url.trim_end_matches('/'));
        let req = reqwest::Client::new().request(method, url);
        match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }
}

pub fn run(command: Command) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match command {
            Command::Apply {
                file,
                format,
                dry_run,
                remote,
            } => apply(&file, format.as_deref(), dry_run, &remote).await,
            Command::Dump { format, remote } => dump(&format, &remote).await,
        }
    })
}

async fn apply(
    file: &Path,
    format: Option<&str>,
    dry_run: bool,
    remote: &Remote,
) -> anyhow::Result<()> {
    let stdin = file == Path::new("-");
    let format = match format {
        Some(name) => {
            ConfigFormat::from_name(name).ok_or_else(|| anyhow!("unknown format '{name}'"))?
        }
        None if stdin => ConfigFormat::Yaml,
        None => file
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ConfigFormat::from_name)
            .ok_or_else(|| anyhow!("can't tell the format of {}; pass --format", file.display()))?,
    };
    let text = if stdin {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text
    } else {
        std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?
    };

    let doc = parse_config(&text, format).map_err(|e| anyhow!("{}: {e}", file.display()))?;
    let mut body = serde_json::to_value(&doc)?;
    expand_env(&mut body, &|name| std::env::var(name).ok())?;

    let path = if dry_run {
        "/config/apply?dry_run=true"
    } else {
        "/config/apply"
    };
    let resp = remote
        .request(reqwest::Method::POST, path)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("connect to {}", remote.url))?;
    let report: Value = ok(resp).await?.json().await?;
    print_report(&report);
    Ok(())
}

async fn dump(format: &str, remote: &Remote) -> anyhow::Result<()> {
    let resp = remote
        .request(reqwest::Method::GET, "/config/current")
        .query(&[("format", format)])
        .send()
        .await
        .with_context(|| format!("connect to {}", remote.url))?;
    print!("{}", ok(resp).await?.text().await?);
    Ok(())
}

/// Turn an error response into its `{"error": ...}` message.
async fn ok(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(body);
    bail!("{status}: {message}")
}

fn print_report(report: &Value) {
    let changes = report["changes"].as_array().cloned().unwrap_or_default();
    for c in &changes {
        let sign = match c["action"].as_str() {
            Some("create") => '+',
            Some("delete") => '-',
            _ => '~',
        };
        let kind = c["kind"].as_str().unwrap_or("").replace('_', " ");
        println!("{sign} {kind} {}", c["name"].as_str().unwrap_or(""));
    }
    let n = changes.len();
    let plural = if n == 1 { "" } else { "s" };
    match (n, report["dry_run"].as_bool().unwrap_or(false)) {
        (0, _) => println!("No changes."),
        (_, true) => println!("Dry run: {n} change{plural} planned, nothing applied."),
        (_, false) => println!("Applied {n} change{plural}."),
    }
    if let Some(note) = report["note"].as_str() {
        println!("Note: {note}");
    }
}

/// Replace `${NAME}` in every string value with the environment variable's
/// value; `$${` writes a literal `${`. Unset variables are an error, and
/// substituted values are not expanded again.
fn expand_env(value: &mut Value, lookup: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<()> {
    match value {
        Value::String(s) => *s = expand_str(s, lookup)?,
        Value::Array(items) => {
            for item in items {
                expand_env(item, lookup)?;
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                expand_env(item, lookup)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn expand_str(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("$${") {
            out.push_str("${");
            rest = r;
        } else if let Some(r) = rest.strip_prefix("${") {
            let end = r
                .find('}')
                .ok_or_else(|| anyhow!("unterminated ${{ in config"))?;
            let name = &r[..end];
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                bail!("invalid variable reference ${{{name}}}");
            }
            let value =
                lookup(name).ok_or_else(|| anyhow!("environment variable {name} is not set"))?;
            out.push_str(&value);
            rest = &r[end + 1..];
        } else {
            out.push('$');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "KEY" => Some("s3cret".into()),
            "TRICKY" => Some("${KEY}".into()),
            _ => None,
        }
    }

    #[test]
    fn expands_references_in_nested_strings() {
        let mut v = json!({"a": ["x-${KEY}-y", {"b": "${KEY}"}], "n": 3, "c": "$5 and $${KEY}"});
        expand_env(&mut v, &lookup).unwrap();
        assert_eq!(
            v,
            json!({"a": ["x-s3cret-y", {"b": "s3cret"}], "n": 3, "c": "$5 and ${KEY}"})
        );
    }

    #[test]
    fn substituted_values_are_not_reexpanded() {
        assert_eq!(expand_str("${TRICKY}", &lookup).unwrap(), "${KEY}");
    }

    #[test]
    fn unset_and_malformed_references_fail() {
        let err = expand_str("${MISSING}", &lookup).unwrap_err().to_string();
        assert!(err.contains("MISSING"), "{err}");
        assert!(expand_str("${KEY", &lookup).is_err());
        assert!(expand_str("${A-B}", &lookup).is_err());
    }
}
