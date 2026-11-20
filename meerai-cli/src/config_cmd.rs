use std::fs;
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use meerai_common::settings::{AppSettings, load_user_settings, save_user_settings};
use meerai_store::Store;
use serde_yaml::Value;

use crate::cli::ConfigCommand;

pub(crate) fn run(command: &ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Export => {
            let settings = load_user_settings(&Store::open_default()?)?;
            let yaml =
                serde_yaml::to_string(&settings).context("serialize user settings as YAML")?;
            io::stdout().lock().write_all(yaml.as_bytes())?;
        }
        ConfigCommand::Import(path) => {
            let yaml = fs::read_to_string(path)
                .with_context(|| format!("read config YAML from {}", path.display()))?;
            let settings = parse_yaml(&yaml, path)?;
            save_user_settings(&Store::open_default()?, &settings)
                .with_context(|| format!("import user settings from {}", path.display()))?;
        }
    }
    Ok(())
}

fn parse_yaml(yaml: &str, path: &Path) -> Result<AppSettings> {
    let value: Value = serde_yaml::from_str(yaml).map_err(|err| match err.location() {
        Some(location) => anyhow!(
            "invalid config YAML in {} at line {}, column {}",
            path.display(),
            location.line(),
            location.column()
        ),
        None => anyhow!("invalid config YAML in {}", path.display()),
    })?;
    if !value.is_mapping() {
        bail!("expected a user settings mapping in {}", path.display());
    }
    serde_yaml::from_value(value)
        .map_err(|_| anyhow!("invalid user settings in {}", path.display()))
}
