// Copyright 2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::io::{read_to_string, stdin, stdout, Seek as _, Write as _};

use anyhow::{bail, Context as _};
use log::{debug, error, info, warn};
use rustix::fs::sync;
use tempfile::NamedTempFile;
use tokio::try_join;
use vstorage::{
    base::{Item, ItemRef, Storage},
    sync::{plan::ItemAction, status::ItemState},
};

use crate::{ConflictResolution, NamedPair, RawCommand};

/// Performs conflict resolution for this storage pair.
///
/// Hint: use the `testing/conflicts.sh` script to interactively test this.
pub async fn interactive_resolution<I: Item>(pair: NamedPair<I>) -> anyhow::Result<()> {
    let raw_cmd = match pair.conflict_resolution {
        Some(ConflictResolution::Cmd(ref rc)) => rc,
        Some(_) => {
            info!("Conflict resolution is automatic for {}.", pair.name);
            return Ok(());
        }
        _ => bail!("No conflict resolution command for {}.", pair.name),
    };

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
    if total == 0 {
        info!("No conflicts to resolve for {}.", pair.name);
        return Ok(());
    }
    println!("Resolving {} conflicts for pair \"{}\".", total, pair.name);

    for (i, (a, b)) in conflicts.into_iter().enumerate() {
        println!("Next is item {}/{total}, with uid \"{}\".", i + 1, a.uid);
        match continue_skip_or_quit()? {
            YesNoQuit::Yes => {}
            YesNoQuit::No => continue,
            YesNoQuit::Quit => {
                println!("Skipping all remaining items for pair \"{}\".", pair.name);
                return Ok(());
            }
        };
        // TODO: should use pre-fetched data, if available.
        // TODO: improve logging here.
        info!("Running conflict resolution for item {}", a.uid);
        let (fetched_a, fetched_b) = tokio::join!(
            fetch_item(pair.inner.storage_a(), a),
            fetch_item(pair.inner.storage_b(), b),
        );
        let (temp_a, item_a, ref_a) = fetched_a.context("fetching conflicted item from A")?;
        let (temp_b, item_b, ref_b) = fetched_b.context("fetching conflicted item from B")?;

        let new = match resolve_individual_conflict(raw_cmd, temp_a, temp_b) {
            Ok(data) => I::from(data),
            Err(err) => {
                error!("Error resolving conflict: {err}");
                continue;
            }
        };

        upload_resolved(&pair, &ref_a, &ref_b, item_a, item_b, &new).await?;

        info!("Resolved conflicts for '{}'.", new.ident());
    }
    println!("No conflicting items left for pair \"{}\".", pair.name);

    Ok(())
}

enum YesNoQuit {
    Yes,
    No,
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
        };
    }
}

/// Returns (file, item, etag).
async fn fetch_item<I: Item>(
    storage: &dyn Storage<I>,
    item: ItemState<I>,
) -> anyhow::Result<(NamedTempFile, I, ItemRef)> {
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

    let item_ref = ItemRef {
        href: item.href,
        etag,
    };
    Ok((temp, data, item_ref))
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
        println!("Resulting item is not identical on both sides. Conflict not resolved.");
        bail!("Conflict resolution yielded mismatching items.");
    }
    Ok(new_a)
}

async fn upload_resolved<I: Item>(
    pair: &NamedPair<I>,
    ref_a: &ItemRef,
    ref_b: &ItemRef,
    orig_a: I,
    orig_b: I,
    new: &I,
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
    };
    Ok(())
}
