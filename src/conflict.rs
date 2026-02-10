// Copyright 2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::io::{Seek as _, Write as _, read_to_string, stdin, stdout};

use anyhow::{Context as _, bail};
use futures_util::StreamExt;
use log::{debug, error, info};
use rustix::fs::sync;
use tempfile::NamedTempFile;
use tokio::try_join;
use vstorage::{
    base::{Item, ItemVersion, Property},
    sync::{
        analysis::{ItemWithData, ResolvedMapping},
        operation::{ItemOp, Operation, PropertyOp},
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
            Ok(Operation::Property(op @ PropertyOp::PropertyConflict { .. })) => {
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
        match continue_skip_or_quit()? {
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

        let (temp_a, ref_a) = write_to_temp(&info.a).context("writing item A to temp file")?;
        let (temp_b, ref_b) = write_to_temp(&info.b).context("writing item B to temp file")?;

        let new = match resolve_individual_item_conflict(raw_cmd, temp_a, temp_b) {
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
        let PropertyOp::PropertyConflict {
            property,
            value_a,
            value_b,
            mapping,
            ..
        } = op
        else {
            unreachable!()
        };

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

enum YesNoQuit {
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

fn continue_skip_or_quit() -> anyhow::Result<YesNoQuit> {
    loop {
        print!("Resolve it manually? (Y)es, (N)o, or (Q)uit? ");
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
                match edit_property_value(value_a, value_b, raw_cmd) {
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

/// Open property values in editor for manual editing.
fn edit_property_value(
    value_a: &str,
    value_b: &str,
    raw_cmd: &RawCommand,
) -> anyhow::Result<String> {
    let mut temp_a = NamedTempFile::new().context("Creating temporary file for A")?;
    let mut temp_b = NamedTempFile::new().context("Creating temporary file for B")?;

    temp_a
        .write_all(value_a.as_bytes())
        .context("writing value A to temp file")?;
    temp_b
        .write_all(value_b.as_bytes())
        .context("writing value B to temp file")?;

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

    sync();
    temp_a.rewind().context("seeking in temporary file for A")?;
    temp_b.rewind().context("seeking in temporary file for B")?;

    let new_a = read_to_string(temp_a).context("reading resolved value A")?;
    let new_b = read_to_string(temp_b).context("reading resolved value B")?;

    let new_a = new_a.trim();
    let new_b = new_b.trim();

    if new_a.is_empty() {
        bail!("Resolved value A is empty.");
    }
    if new_b.is_empty() {
        bail!("Resolved value B is empty.");
    }
    if new_a != new_b {
        println!("Values are not identical. Please make both files the same.");
        bail!("Edited values don't match.");
    }

    Ok(new_a.to_string())
}

/// Write item data to a temporary file and return the file and version info.
fn write_to_temp(item: &ItemWithData) -> anyhow::Result<(NamedTempFile, ItemVersion)> {
    let mut temp = NamedTempFile::new().context("Creating temporary file.")?;
    temp.write_all(item.data.as_str().as_bytes())
        .context("writing item into temporary file")?;

    let item_ver = item.state.version.clone();
    Ok((temp, item_ver))
}

/// Returns `None` if resolution failed.
fn resolve_individual_item_conflict(
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

    // Ensure that files are committed; otherwise we sometimes read empty data.
    sync();
    temp_a.rewind().context("seeking in temporary file for A")?;
    temp_b.rewind().context("seeking in temporary file for B")?;

    let new_a = read_to_string(temp_a).context("reading resolved item A")?;
    let new_b = read_to_string(temp_b).context("reading resolved item B")?;

    if new_a.is_empty() {
        bail!("Resolved item A is empty.");
    }
    if new_b.is_empty() {
        bail!("Resolved item B is empty.");
    }
    if new_a.trim() != new_b.trim() {
        println!("Resulting item is not identical on both sides. Conflict not resolved.");
        bail!("Conflict resolution yielded mismatching items.");
    }
    Ok(new_a)
}

async fn upload_resolved_item(
    pair: &NamedPair,
    ref_a: &ItemVersion,
    ref_b: &ItemVersion,
    orig_a: &Item,
    orig_b: &Item,
    new: &Item,
) -> anyhow::Result<()> {
    let mut task_a = None;
    let mut task_b = None;

    if new.hash() == orig_a.hash() {
        debug!("Item is unchanged in A.");
    } else {
        task_a = Some(async {
            pair.inner
                .storage_a()
                .update_item(&ref_a.href, &ref_a.etag, new)
                .await
                .context("uploading resolved item into A")
        });
    }

    if new.hash() == orig_b.hash() {
        debug!("Item is unchanged in B.");
    } else {
        task_b = Some(async {
            pair.inner
                .storage_b()
                .update_item(&ref_b.href, &ref_b.etag, new)
                .await
                .context("uploading resolved item into B")
        });
    }

    match (task_a, task_b) {
        (None, None) => {}
        (None, Some(b)) => {
            b.await?;
            debug!("Uploaded resolved item to B.");
        }
        (Some(a), None) => {
            a.await?;
            debug!("Uploaded resolved item to A.");
        }
        (Some(a), Some(b)) => {
            try_join!(a, b)?;
            debug!("Uploaded resolved items.");
        }
    }
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
    let mut task_a = None;
    let mut task_b = None;

    if new_value == orig_a {
        debug!("Property is unchanged in A.");
    } else {
        task_a = Some(async {
            pair.inner
                .storage_a()
                .set_property(mapping.a().href(), *property, new_value)
                .await
                .context("setting resolved property on A")
        });
    }

    if new_value == orig_b {
        debug!("Property is unchanged in B.");
    } else {
        task_b = Some(async {
            pair.inner
                .storage_b()
                .set_property(mapping.b().href(), *property, new_value)
                .await
                .context("setting resolved property on B")
        });
    }

    match (task_a, task_b) {
        (None, None) => {}
        (None, Some(b)) => {
            b.await?;
            debug!("Set resolved property on B.");
        }
        (Some(a), None) => {
            a.await?;
            debug!("Set resolved property on A.");
        }
        (Some(a), Some(b)) => {
            try_join!(a, b)?;
            debug!("Set resolved property on both storages.");
        }
    }
    Ok(())
}
