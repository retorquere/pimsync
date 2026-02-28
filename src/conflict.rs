// Copyright 2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::io::{Seek as _, Write as _, read_to_string, stdin, stdout};

use anyhow::{Context as _, bail};
use futures_util::StreamExt;
use futures_util::future::OptionFuture;
use log::{error, info};
use rustix::fs::sync;
use tempfile::NamedTempFile;
use vstorage::{
    base::{Item, ItemVersion},
    property::Property,
    sync::{
        analysis::ResolvedMapping,
        operation::{ItemOp, Operation, PropertyOp, PropertyOpKind},
    },
};

use crate::{ConflictResolution, NamedPair, RawCommand};

/// Performs conflict resolution for this storage pair.
///
/// Hint: use the `testing/conflicts.sh` script to interactively test this.
pub async fn interactive_resolution(pair: NamedPair) -> anyhow::Result<()> {
    let raw_cmd = match pair.conflict_resolution {
        Some(ConflictResolution::Cmd(ref rc)) => rc,
        Some(_) => {
            info!("Conflict resolution is automatic for {}.", pair.name);
            return Ok(());
        }
        _ => bail!("No conflict resolution command for {}.", pair.name),
    };

    let mut plan = pair.create_plan().await?;

    // Collect in order to compute the total amount (for display purposes).
    let mut item_conflicts = Vec::new();
    let mut property_conflicts = Vec::new();
    while let Some(result) = plan.next().await {
        match result {
            Ok(Operation::Item(ItemOp::Conflict { info, .. })) => {
                item_conflicts.push(info);
            }
            Ok(Operation::Property(
                op @ PropertyOp {
                    kind: PropertyOpKind::Conflict { .. },
                    ..
                },
            )) => {
                property_conflicts.push(op);
            }
            Ok(_) => {} // Skip non-conflict operations
            Err(err) => {
                error!("Error in plan: {err:?}");
            }
        }
    }

    let item_total = item_conflicts.len();
    let property_total = property_conflicts.len();
    if item_total == 0 && property_total == 0 {
        info!("No conflicts to resolve for {}.", pair.name);
        return Ok(());
    }
    println!(
        "Resolving {} item conflicts and {} property conflicts for pair \"{}\".",
        item_total, property_total, pair.name
    );

    if !resolve_item_conflicts(&pair, item_conflicts, raw_cmd).await? {
        return Ok(());
    }
    resolve_property_conflicts(&pair, property_conflicts, raw_cmd).await?;

    println!("No conflicts left for pair \"{}\".", pair.name);

    Ok(())
}

/// Resolve item conflicts interactively. Returns `false` if user chose to quit.
async fn resolve_item_conflicts(
    pair: &NamedPair,
    conflicts: Vec<vstorage::sync::conflict::ConflictInfo>,
    raw_cmd: &RawCommand,
) -> anyhow::Result<bool> {
    let total = conflicts.len();
    for (i, info) in conflicts.into_iter().enumerate() {
        println!(
            "Next is item {}/{total}, with uid \"{}\".",
            i + 1,
            info.a.state.uid
        );
        match continue_skip_or_quit("Resolve it manually?")? {
            YesNoQuit::Yes => {}
            YesNoQuit::No => continue,
            YesNoQuit::Quit => {
                println!(
                    "Skipping all remaining conflicts for pair \"{}\".",
                    pair.name
                );
                return Ok(false);
            }
        }
        info!("Running conflict resolution for item {}", info.a.state.uid);

        let temp_a = write_to_temp_file(info.a.data.as_str().as_bytes())
            .context("writing item A to temp file")?;
        let temp_b = write_to_temp_file(info.b.data.as_str().as_bytes())
            .context("writing item B to temp file")?;
        let ref_a = info.a.state.version.clone();
        let ref_b = info.b.state.version.clone();

        let new = match run_merge_command(raw_cmd, temp_a, temp_b) {
            Ok(data) => Item::from(data),
            Err(err) => {
                error!("Error resolving conflict: {err}");
                continue;
            }
        };

        upload_resolved_item(pair, &ref_a, &ref_b, &info.a.data, &info.b.data, &new).await?;

        info!("Resolved conflict for '{}'.", new.ident());
    }
    Ok(true)
}

/// Resolve property conflicts interactively.
async fn resolve_property_conflicts(
    pair: &NamedPair,
    conflicts: Vec<PropertyOp>,
    raw_cmd: &RawCommand,
) -> anyhow::Result<()> {
    let total = conflicts.len();
    for (i, op) in conflicts.into_iter().enumerate() {
        let PropertyOpKind::Conflict { value_a, value_b } = op.kind else {
            unreachable!()
        };
        let property = op.property;
        let mapping = op.mapping;

        println!(
            "Property {}/{total}: \"{}\" on collection \"{}\":",
            i + 1,
            property.name(),
            mapping.alias()
        );
        println!("  A: \"{value_a}\"");
        println!("  B: \"{value_b}\"");

        let resolved = match choose_property_value(&value_a, &value_b, raw_cmd)? {
            PropertyChoice::A => value_a.clone(),
            PropertyChoice::B => value_b.clone(),
            PropertyChoice::Custom(value) => value,
            PropertyChoice::Skip => continue,
            PropertyChoice::Quit => {
                println!(
                    "Skipping all remaining properties for pair \"{}\".",
                    pair.name
                );
                break;
            }
        };

        info!("Resolved property '{}' to '{}'", property.name(), resolved);

        upload_resolved_property(pair, &property, &mapping, &value_a, &value_b, &resolved).await?;
    }
    Ok(())
}

pub enum YesNoQuit {
    Yes,
    No,
    Quit,
}

enum PropertyChoice {
    A,
    B,
    Custom(String),
    Skip,
    Quit,
}

pub fn continue_skip_or_quit(msg: &str) -> anyhow::Result<YesNoQuit> {
    loop {
        print!("{msg} (Y)es, (N)o, or (Q)uit? ");
        stdout().flush()?;
        // Need to read entire lines because the stdlib implicitly buffers stdin.
        let mut response = String::new();
        stdin()
            .read_line(&mut response)
            .context("Reading response from stdin")?;

        match response.trim().to_lowercase().as_str() {
            "" | "y" => return Ok(YesNoQuit::Yes),
            "n" => return Ok(YesNoQuit::No),
            "q" => return Ok(YesNoQuit::Quit),
            _ => {}
        }
    }
}

/// Prompt user to choose a property value: keep A, keep B, edit, skip, or quit.
fn choose_property_value(
    value_a: &str,
    value_b: &str,
    raw_cmd: &RawCommand,
) -> anyhow::Result<PropertyChoice> {
    loop {
        print!("Keep (A), (B), (E)dit, (S)kip, or (Q)uit? ");
        stdout().flush()?;

        let mut response = String::new();
        stdin()
            .read_line(&mut response)
            .context("Reading response from stdin")?;

        match response.trim().to_lowercase().as_str() {
            "a" => return Ok(PropertyChoice::A),
            "b" => return Ok(PropertyChoice::B),
            "e" => {
                let temp_a = write_to_temp_file(value_a.as_bytes())?;
                let temp_b = write_to_temp_file(value_b.as_bytes())?;
                match run_merge_command(raw_cmd, temp_a, temp_b).map(|s| s.trim().to_string()) {
                    Ok(value) => return Ok(PropertyChoice::Custom(value)),
                    Err(err) => {
                        error!("Error editing property: {err}");
                        // Let user try again
                    }
                }
            }
            "s" => return Ok(PropertyChoice::Skip),
            "q" => return Ok(PropertyChoice::Quit),
            _ => {}
        }
    }
}

/// Run a merge command on two temporary files and validate the result.
///
/// Execute the command with both file paths as arguments. After the command exits,
/// read both files back and validate that they are non-empty and identical.
fn run_merge_command(
    raw_cmd: &RawCommand,
    mut temp_a: NamedTempFile,
    mut temp_b: NamedTempFile,
) -> anyhow::Result<String> {
    let exit_status = raw_cmd
        .command()
        .arg(temp_a.path())
        .arg(temp_b.path())
        .spawn()
        .context("executing conflict resolution command")?
        .wait()
        .context("waiting for conflict resolution command")?;

    if !exit_status.success() {
        bail!("Conflict resolution command failed: {exit_status}");
    }

    // Ensure that files are committed to disk; otherwise we sometimes read empty data.
    sync();
    temp_a.rewind().context("seeking in temporary file for A")?;
    temp_b.rewind().context("seeking in temporary file for B")?;

    let new_a = read_to_string(temp_a).context("reading resolved value A")?;
    let new_b = read_to_string(temp_b).context("reading resolved value B")?;

    if new_a.trim().is_empty() {
        bail!("Resolved value A is empty.");
    }
    if new_b.trim().is_empty() {
        bail!("Resolved value B is empty.");
    }
    if new_a.trim() != new_b.trim() {
        println!("Resulting values are not identical on both sides.");
        bail!("Conflict resolution yielded mismatching values.");
    }
    Ok(new_a)
}

/// Write data to a new temporary file.
fn write_to_temp_file(data: &[u8]) -> anyhow::Result<NamedTempFile> {
    let mut temp = NamedTempFile::new().context("Creating temporary file")?;
    temp.write_all(data)
        .context("writing into temporary file")?;
    Ok(temp)
}

async fn upload_resolved_item(
    pair: &NamedPair,
    ref_a: &ItemVersion,
    ref_b: &ItemVersion,
    orig_a: &Item,
    orig_b: &Item,
    new: &Item,
) -> anyhow::Result<()> {
    let task_a: OptionFuture<_> = (new.hash() != orig_a.hash())
        .then_some(async {
            pair.inner
                .storage_a()
                .update_item(&ref_a.href, &ref_a.etag, new)
                .await
                .context("uploading resolved item into A")
        })
        .into();
    let task_b: OptionFuture<_> = (new.hash() != orig_b.hash())
        .then_some(async {
            pair.inner
                .storage_b()
                .update_item(&ref_b.href, &ref_b.etag, new)
                .await
                .context("uploading resolved item into B")
        })
        .into();
    let (result_a, result_b) = tokio::join!(task_a, task_b);
    result_a.transpose()?;
    result_b.transpose()?;
    Ok(())
}

async fn upload_resolved_property(
    pair: &NamedPair,
    property: &Property,
    mapping: &ResolvedMapping,
    orig_a: &str,
    orig_b: &str,
    new_value: &str,
) -> anyhow::Result<()> {
    let task_a: OptionFuture<_> = (new_value != orig_a)
        .then_some(async {
            pair.inner
                .storage_a()
                .set_property(mapping.a().href(), *property, new_value)
                .await
                .context("setting resolved property on A")
        })
        .into();
    let task_b: OptionFuture<_> = (new_value != orig_b)
        .then_some(async {
            pair.inner
                .storage_b()
                .set_property(mapping.b().href(), *property, new_value)
                .await
                .context("setting resolved property on B")
        })
        .into();
    let (result_a, result_b) = tokio::join!(task_a, task_b);
    result_a.transpose()?;
    result_b.transpose()?;
    Ok(())
}
