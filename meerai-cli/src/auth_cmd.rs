use std::io::{self, Write as _};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use meerai_client::providers::{ProviderCatalog, canonical_provider_id, format_auth_list};
use meerai_client::secrets::{Credential, SecretStore};
use meerai_common::settings::{AppSettings, save_settings_to};
use meerai_providers::models::fetch_plan_models;
use meerai_providers::oauth::{LoginInteraction, login_chatgpt, revoke_chatgpt_credential};

use crate::cli::AuthCommand;

pub(crate) async fn run_auth(
    auth_cmd: AuthCommand,
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
    auth_path: Option<&Path>,
    config_dir: Option<&Path>,
    data_dir: Option<&Path>,
    settings: &mut AppSettings,
) -> Result<()> {
    match auth_cmd {
        AuthCommand::List => {
            println!(
                "{}",
                format_auth_list(catalog, secrets, auth_path, config_dir)
            );
        }
        AuthCommand::Login {
            provider,
            key,
            chatgpt,
        } => {
            let provider_id = canonical_provider_id(&provider);
            if chatgpt {
                return login_with_chatgpt(
                    &provider_id,
                    catalog,
                    secrets,
                    config_dir,
                    data_dir,
                    settings,
                )
                .await;
            }
            let api_key = match key {
                Some(k) => k,
                None => {
                    print!("API key for {provider_id}: ");
                    io::stdout().flush()?;
                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    input
                }
            };
            secrets.set_key(&provider_id, &api_key)?;
            let target = auth_path
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "auth.json".to_string());
            println!("Saved credentials for {provider_id} in {target}");
        }
        AuthCommand::Logout { provider } => {
            let provider_id = canonical_provider_id(&provider);
            log_out(&provider_id, secrets).await?;
            println!("Removed stored credentials for {provider_id}");
        }
    }
    Ok(())
}

async fn login_with_chatgpt(
    provider_id: &str,
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
    config_dir: Option<&Path>,
    data_dir: Option<&Path>,
    settings: &mut AppSettings,
) -> Result<()> {
    let provider = catalog
        .provider(provider_id)
        .with_context(|| format!("unknown provider '{provider_id}'"))?;
    if !provider.auth.supports_oauth() {
        bail!("provider '{provider_id}' does not support ChatGPT plan sign-in");
    }
    if provider_id != "openai" {
        bail!("ChatGPT plan sign-in is only supported for the openai provider");
    }
    let data_dir =
        data_dir.context("no data directory; set HOME or XDG_DATA_HOME before signing in")?;

    let credential = login_chatgpt(secrets, provider_id, data_dir, &CliLogin).await?;
    secrets.write_credential(provider_id, &Credential::OAuth(credential.clone()))?;
    let expiry = credential
        .expires
        .saturating_sub(now_ms())
        .saturating_div(60_000);
    println!(
        "Signed in with ChatGPT{}. Plan token expires in {expiry}m.",
        credential
            .account
            .as_deref()
            .map(|account| format!(" as {account}"))
            .unwrap_or_default()
    );

    match fetch_plan_models(&credential.access).await {
        Ok(models) if !models.is_empty() => {
            println!("Available plan models:");
            for model in &models {
                println!("  {}  {}", model.id, model.display_name);
            }
            for model in models.iter().take(10) {
                settings.record_provider_model(provider_id, &model.id);
            }
            if let Some(dir) = config_dir
                && let Err(err) = save_settings_to(dir, settings)
            {
                eprintln!("Warning: could not save recent models: {err:#}");
            }
        }
        Ok(_) => {
            println!("No plan models were listed; enter a model name manually with /model.");
        }
        Err(err) => {
            eprintln!("Warning: could not list plan models: {err:#}");
        }
    }
    Ok(())
}

async fn log_out(provider_id: &str, secrets: &dyn SecretStore) -> Result<()> {
    match secrets.read_credential(provider_id)? {
        Some(Credential::OAuth(_)) => revoke_chatgpt_credential(secrets, provider_id).await,
        Some(Credential::ApiKey(_)) => secrets.delete_key(provider_id),
        None => secrets.delete_credential(provider_id),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

struct CliLogin;

#[async_trait::async_trait]
impl LoginInteraction for CliLogin {
    async fn present_url(&self, url: &str) -> Result<()> {
        println!("Open this URL in your browser to sign in with ChatGPT:");
        println!("  {url}");
        println!();
        let _ = open_browser(url);
        println!("Waiting for the browser callback. You can also paste the final redirect URL.");
        Ok(())
    }

    async fn read_pasted_input(&self) -> Result<String> {
        tokio::task::spawn_blocking(|| {
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            Ok::<String, io::Error>(input)
        })
        .await
        .context("paste prompt task failed")?
        .context("failed to read the pasted callback URL")
    }
}

pub(crate) fn open_browser(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]).arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
