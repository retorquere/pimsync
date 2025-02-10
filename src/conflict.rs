// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::io::{read_to_string, stdin, Seek as _, Write as _};

use anyhow::{bail, Context as _};
use log::{debug, error, info, warn};
use rustix::fs::sync;
use tempfile::NamedTempFile;
use vstorage::{
    base::{Item, Storage},
    sync::{plan::ItemAction, status::ItemState},
    Etag,
};

use crate::{ConflictResolution, NamedPair, RawCommand};

/// Performs conflict resolution for this storage pair.
// TODO: The UI can run in a deducted thread, taking a channel of conflicts to be resolved.
//       All the related I/O would continue on other threads, asynchronously.
pub async fn interactive_resolution<I: Item>(pair: NamedPair<I>) -> anyhow::Result<()> {
    // TODO: are storage locks necessary here?
    let raw_cmd = match pair.conflict_resolution {
        Some(ConflictResolution::Cmd(ref rc)) => rc,
        Some(_) => bail!("Conflict resolution is automatic for {}.", pair.name),
        _ => bail!("No conflict resolution command for {}.", pair.name),
    };
    info!("Resolving conflicts for pair {}.", pair.name);

    let plan = pair.create_plan().await?;
    pair.print_plan(&plan);

    let conflicts = plan
        .collection_plans
        .into_iter()
        .flat_map(|cp| cp.items)
        .filter_map(|action| match action {
            ItemAction::Conflict { a, b, .. } => Some((a, b)),
            _ => None,
        })
        // Collect in order to compute the total amount (for display purposes).
        .collect::<Vec<_>>();

    let total = conflicts.len();

    for (i, (a, b)) in conflicts.into_iter().enumerate() {
        println!("Next is item {}/{total}", i + 1);
        continue_or_abort()?;

        // TODO: should use pre-fetched data, if available.
        // TODO: improve logging here.
        let (fetched_a, fetched_b) = tokio::join!(
            fetch_item(pair.inner.storage_a(), &a),
            fetch_item(pair.inner.storage_b(), &b),
        );
        let (temp_a, item_a, etag_a) = fetched_a.context("fetching conflicted item from A")?;
        let (temp_b, item_b, etag_b) = fetched_b.context("fetching conflicted item from B")?;

        info!("Running conflict resolution for item {}", a.uid);
        let new = match resolve_individual_conflict(raw_cmd, temp_a, temp_b) {
            Ok(data) => I::from(data),
            Err(err) => {
                error!("Error resolving conflict: {err}");
                continue;
            }
        };

        if new.hash() == item_a.hash() {
            debug!("Item is unchanged in A.");
        } else {
            pair.inner
                .storage_a()
                .update_item(&a.href, &etag_a, &new)
                .await
                .context("uploading resolved item into A")?;
            debug!("Uploaded resolved item to A.");
        }

        if new.hash() == item_b.hash() {
            debug!("Item is unchanged in B.");
        } else {
            pair.inner
                .storage_b()
                .update_item(&b.href, &etag_b, &new)
                .await
                .context("uploading resolved item into B")?;
            debug!("Uploaded resolved item to B.");
        }

        info!("Resolved conflicts for '{}'.", new.ident());
    }

    Ok(())
}

/// Returns an error if user chooses to abort.
fn continue_or_abort() -> anyhow::Result<()> {
    loop {
        println!("Continue? [Y/n]");
        // Need to read entire lines because the stdlib implicitly buffers stdin.
        let mut response = String::new();
        stdin()
            .read_line(&mut response)
            .context("Reading response from stdin")?;

        match response.trim().to_lowercase().as_str() {
            "" | "y" => return Ok(()),
            "n" => bail!("Aborted by user."),
            _ => {}
        };
    }
}

/// Returns (file, item, etag).
async fn fetch_item<I: Item>(
    storage: &dyn Storage<I>,
    item: &ItemState<I>,
) -> anyhow::Result<(NamedTempFile, I, Etag)> {
    let mut temp = NamedTempFile::new().context("Creating temporary file.")?;
    debug!("Fetching {} for conflict resolution...", item.href);
    let (data, etag) = if let Some(ref i) = item.data {
        warn!("Conflicted item was not pre-fetched");
        (i.clone(), item.etag.clone())
    } else {
        storage
            .get_item(&item.href)
            .await
            .context("Fetching conflicting item from A.")?
    };
    temp.write_all(data.as_str().as_bytes())
        .context("writing item into temporary file")?;

    Ok((temp, data, etag))
}

/// Returns `None` if resolution failed.
fn resolve_individual_conflict(
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
        bail!("Conflict resolution yielded mismatching items; skipping");
    }
    Ok(new_a)
}
