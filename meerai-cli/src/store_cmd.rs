use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use meerai_common::settings::find_project_root;
use meerai_store::{ModelRoute, NewProjectMemory, ProjectMemoryKind, Store, TaskType};

use crate::cli::{MemoryCommand, PackCommand, RoutesCommand};

pub(crate) fn run_memory(command: MemoryCommand) -> Result<()> {
    let store = Store::open_default()?;
    let repo = store.memories();
    match command {
        MemoryCommand::List { project, query } => {
            let project = project_root(project);
            let memories = match query {
                Some(query) => repo.search(&project, &query)?,
                None => repo.list(&project)?,
            };
            if memories.is_empty() {
                println!("No project memories.");
                return Ok(());
            }
            for memory in memories {
                println!(
                    "{} [{}]{}\t{}",
                    memory.id,
                    kind_label(memory.kind),
                    if memory.pinned { " (pinned)" } else { "" },
                    memory.title
                );
            }
        }
        MemoryCommand::Save {
            project,
            kind,
            title,
            body,
            pinned,
        } => {
            let project = project_root(project);
            let kind = ProjectMemoryKind::parse(&kind)?;
            let memory = repo.insert(&NewProjectMemory {
                project_root: project,
                kind,
                title,
                body: body.unwrap_or_default(),
                source_session_id: None,
                pinned,
            })?;
            println!("Saved memory {}", memory.id);
        }
        MemoryCommand::Delete { project, id } => {
            let removed = repo.delete(&project_root(project), &id)?;
            println!(
                "{}",
                if removed {
                    format!("Removed memory {id}")
                } else {
                    format!("No memory {id}")
                }
            );
        }
        MemoryCommand::Pin {
            project,
            id,
            pinned,
        } => {
            let memory = repo
                .set_pinned(&project_root(project), &id, pinned)?
                .with_context(|| format!("no memory {id}"))?;
            println!(
                "Memory {} is {}",
                memory.id,
                if memory.pinned { "pinned" } else { "unpinned" }
            );
        }
    }
    Ok(())
}

pub(crate) fn run_routes(command: RoutesCommand) -> Result<()> {
    let store = Store::open_default()?;
    let repo = store.routes();
    match command {
        RoutesCommand::List => {
            let routes = repo.load()?;
            if routes.routes.is_empty() {
                println!("No model routes.");
                return Ok(());
            }
            for route in routes.routes {
                println!(
                    "{} -> {}::{}",
                    route.task_type.as_key(),
                    route.provider,
                    route.model
                );
            }
        }
        RoutesCommand::Set {
            task,
            spec,
            effort,
            max_cost_usd,
            max_tokens_per_turn,
            max_concurrency,
        } => {
            let (provider, model) = spec
                .split_once("::")
                .or_else(|| spec.split_once(':'))
                .with_context(|| format!("route model must be provider::model: {spec}"))?;
            if let Some(level) = effort.as_deref()
                && !matches!(level, "low" | "medium" | "high")
            {
                bail!("unknown effort level: {level}");
            }
            let route = ModelRoute {
                task_type: TaskType::parse(&task)?,
                provider: provider.to_string(),
                model: model.to_string(),
                effort,
                max_tokens_per_turn,
                max_cost_usd,
                max_concurrency,
            };
            repo.upsert(&route)?;
            println!("Saved route {task}");
        }
        RoutesCommand::Clear { task } => {
            let removed = repo.remove(&TaskType::parse(&task)?)?;
            println!(
                "{}",
                if removed {
                    format!("Removed route {task}")
                } else {
                    format!("No route {task}")
                }
            );
        }
    }
    Ok(())
}

pub(crate) fn run_pack(command: PackCommand) -> Result<()> {
    let store = Store::open_default()?;
    let repo = store.packs();
    match command {
        PackCommand::Add { source, rev } => {
            let pack = meerai_common::packs::install_pack(&source, rev.as_deref())?;
            repo.upsert(&pack)?;
            println!("Installed pack {} at {}", pack.id, pack.pinned_rev);
        }
        PackCommand::List => {
            let packs = repo.list()?;
            if packs.is_empty() {
                println!("No installed packs.");
                return Ok(());
            }
            for pack in packs {
                println!(
                    "{} [{}] [{}] rev {} from {}",
                    pack.id,
                    if pack.enabled { "enabled" } else { "disabled" },
                    if pack.trusted { "trusted" } else { "untrusted" },
                    pack.pinned_rev,
                    pack.source_uri
                );
            }
        }
        PackCommand::SetEnabled { id, enabled } => {
            let changed = repo.set_enabled(&id, enabled)?;
            println!(
                "{}",
                if changed {
                    format!(
                        "Pack {id} is {}",
                        if enabled { "enabled" } else { "disabled" }
                    )
                } else {
                    format!("No pack {id}")
                }
            );
        }
        PackCommand::SetTrusted { id, trusted } => {
            let changed = repo.set_trusted(&id, trusted)?;
            println!(
                "{}",
                if changed {
                    format!(
                        "Pack {id} is {}",
                        if trusted { "trusted" } else { "untrusted" }
                    )
                } else {
                    format!("No pack {id}")
                }
            );
        }
        PackCommand::Pin { id, rev } => {
            let changed = repo.pin(&id, &rev)?;
            println!(
                "{}",
                if changed {
                    format!("Pinned pack {id} at {rev}")
                } else {
                    format!("No pack {id}")
                }
            );
        }
        PackCommand::Remove { id } => {
            let removed = repo.remove(&id)?;
            println!(
                "{}",
                if removed {
                    format!("Removed pack {id}")
                } else {
                    format!("No pack {id}")
                }
            );
        }
    }
    Ok(())
}

fn project_root(explicit: Option<String>) -> String {
    explicit.unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        find_project_root(&cwd).display().to_string()
    })
}

fn kind_label(kind: ProjectMemoryKind) -> &'static str {
    match kind {
        ProjectMemoryKind::Decision => "decision",
        ProjectMemoryKind::Progress => "progress",
        ProjectMemoryKind::Fact => "fact",
        ProjectMemoryKind::Convention => "convention",
    }
}
