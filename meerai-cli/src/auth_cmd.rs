use std::io::{self, Write as _};
use std::path::Path;

use anyhow::Result;
use meerai_providers::providers::{ProviderCatalog, canonical_provider_id, format_auth_list};
use meerai_providers::secrets::SecretStore;

use crate::cli::AuthCommand;

pub(crate) fn run_auth(
    auth_cmd: AuthCommand,
    catalog: &ProviderCatalog,
    secrets: &dyn SecretStore,
    auth_path: Option<&Path>,
    config_dir: Option<&Path>,
) -> Result<()> {
    match auth_cmd {
        AuthCommand::List => {
            println!(
                "{}",
                format_auth_list(catalog, secrets, auth_path, config_dir)
            );
        }
        AuthCommand::Login { provider, key } => {
            let provider_id = canonical_provider_id(&provider);
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
            secrets.delete_key(&provider_id)?;
            println!("Removed stored credentials for {provider_id}");
        }
    }
    Ok(())
}
