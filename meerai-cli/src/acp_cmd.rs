use std::path::Path;

use anyhow::Result;

use crate::cli::AcpCommand;

pub(crate) async fn run(command: AcpCommand, workspace_root: &Path) -> Result<()> {
    match command {
        AcpCommand::Doctor => match doctor(workspace_root).await {
            Ok(()) => Ok(()),
            Err(err) => {
                eprintln!("acp doctor failed: {err:#}");
                Err(err)
            }
        },
    }
}

async fn doctor(workspace_root: &Path) -> Result<()> {
    let preset = meerai_acp::AGY;
    let session = meerai_acp::connect(preset, workspace_root.to_path_buf()).await?;
    let handshake = session.handshake().clone();
    let stderr = session.stderr_tail();

    let agent = format!(
        "{} {}",
        handshake.agent_name.as_deref().unwrap_or("unknown"),
        handshake.agent_version.as_deref().unwrap_or("unknown")
    );
    println!("preset: {}", preset.id);
    println!("command: {} {}", preset.command, preset.args.join(" "));
    println!("protocol: {}", handshake.protocol_version);
    println!("agent: {agent}");
    println!("load_session: {}", handshake.load_session);
    println!("auth: {}", join_or_none(&handshake.auth_methods));
    println!("modes: {}", join_or_none(&handshake.modes));
    println!("session: {}", handshake.session_id);
    println!(
        "stderr: {}",
        if stderr.is_empty() {
            "none".to_string()
        } else {
            stderr.join("\n")
        }
    );

    session.shutdown().await?;
    Ok(())
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}
