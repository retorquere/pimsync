// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::io::{read_to_string, BufRead as _, Seek as _, StdinLock, Write as _};

use anyhow::{bail, Context as _};
use log::{debug, error, info};
use rustix::fs::sync;
use tempfile::NamedTempFile;
use vstorage::{
    base::{Item, Storage},
    sync::plan::ItemAction,
    Etag,
};

use crate::{ConflictResolution, NamedPair};

/// Performs conflict resolution for this storage pair.
// TODO: The UI can run in a deducted thread, taking a channel of conflicts to be resolved.
//       All the related I/O would continue on other threads, asynchronously.
pub async fn interactive_resolution<I: Item>(
    pair: NamedPair<I>,
    stdin_lock: &mut StdinLock<'_>,
) -> anyhow::Result<()> {
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
        .flat_map(|cp| cp.item_actions)
        .filter_map(|action| match action {
            ItemAction::Conflict { a, b, .. } => Some((a, b)),
            _ => None,
        })
        // Collect in order to compute the total amount (for display purposes).
        .collect::<Vec<_>>();

    let total = conflicts.len();

    for (i, (a, b)) in conflicts.into_iter().enumerate() {
        println!("Next is item {}/{total}", i + 1);
        continue_or_abort(stdin_lock)?;

        // TODO: should use pre-fetched data, if available.
        // TODO: improve logging here.
        // TODO: move duplicated logic into a "read_item_to_tempfile" function.
        let (mut temp_a, etag_a) = save_item_to_tempfile(pair.inner.storage_a(), &a.href)
            .await
            .context("fetching conflicted item from A")?;
        let (mut temp_b, etag_b) = save_item_to_tempfile(pair.inner.storage_b(), &b.href)
            .await
            .context("fetching conflicted item from B")?;

        info!("Running conflict resolution for item {}", a.uid);
        let exit_status = raw_cmd
            .command()
            .arg(temp_a.path())
            .arg(temp_b.path())
            .spawn()
            .context("executing conflict resolution command")?
            .wait()
            .context("waiting for conflict resolution command")?;

        if !exit_status.success() {
            error!("Conflict resolution command failed: {exit_status}");
            continue;
        }

        // Ensure that files are committed; otherwise we sometimes read empty data.
        sync();
        temp_a.rewind().context("seeking in temporary file for A")?;
        temp_b.rewind().context("seeking in temporary file for B")?;

        let new_a = read_to_string(temp_a).context("reading resolved item A")?;
        let new_b = read_to_string(temp_b).context("reading resolved item B")?;

        if new_a.is_empty() {
            error!("Resolved item A is empty.");
            continue;
        }
        if new_b.is_empty() {
            error!("Resolved item B is empty.");
            continue;
        }
        if new_a.trim() != new_b.trim() {
            error!("Conflict resolution yielded mismatching items; skipping");
            continue;
        }

        let new = I::from(new_a);
        drop(new_b);

        pair.inner
            .storage_a()
            .update_item(&a.href, &etag_a, &new)
            .await
            .context("uploading resolved item into A")?;
        debug!("Uploaded resolved item to A.");

        pair.inner
            .storage_b()
            .update_item(&b.href, &etag_b, &new)
            .await
            .context("uploading resolved item into B")?;
        debug!("Uploaded resolved item to B.");

        info!("Resolved conflicts for '{}'.", new.ident());
    }

    Ok(())
}

/// Returns an error if user chooses to abort.
fn continue_or_abort(stdin: &mut StdinLock) -> anyhow::Result<()> {
    loop {
        println!("Continue? [Y/n]");
        // Need to read entire lines because the stdlib implicitly buffers stdin.
        let mut response = String::new();
        stdin
            .read_line(&mut response)
            .context("Reading response from stdin")?;

        match response.trim().to_lowercase().as_str() {
            "" | "y" => return Ok(()),
            "n" => bail!("Aborted by user."),
            _ => {}
        };
    }
}

async fn save_item_to_tempfile<I: Item>(
    storage: &dyn Storage<I>,
    href: &str,
) -> anyhow::Result<(NamedTempFile, Etag)> {
    let mut temp =
        NamedTempFile::new().context("creating temporary file for conflict resolution")?;
    debug!("Fetching {href} for conflict resolution...");
    let (data, etag) = storage
        .get_item(href)
        .await
        .context("fetching conflicting item from a")?;
    temp.write_all(data.as_str().as_bytes())
        .context("writing item into temporary file")?;

    Ok((temp, etag))
}
