use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::bail;
use log::{error, info};
use tokio::task::JoinSet;
use vstorage::base::Storage;

/// Storage with a name (used for logging).
pub struct NamedStorage {
    pub name: String,
    pub storage: Arc<dyn Storage>,
}

/// Main entry point to repairing storages.
pub(crate) async fn repair_storages(storages: Vec<NamedStorage>) -> anyhow::Result<()> {
    let failed = Arc::new(AtomicU32::new(0));
    let mut tasks = storages
        .into_iter()
        .map(|storage| repair_storage(storage, failed.clone()))
        .collect::<JoinSet<_>>();
    while let Some(res) = tasks.join_next().await {
        if let Err(joinerr) = res {
            error!("Error in task for storage: {joinerr}");
        }
    }
    match failed.load(Ordering::Relaxed) {
        0 => {
            info!("Repair complete, no failures.");
            Ok(())
        }
        some => bail!("A total of {some} operations failed."),
    }
}

async fn repair_storage(storage: NamedStorage, failed: Arc<AtomicU32>) {
    let NamedStorage { name, storage } = storage;
    info!("Repairing storage {name}");
    let disco = match storage.discover_collections().await {
        Ok(d) => d,
        Err(err) => {
            error!("Discovery failed for storage {name}: {err}");
            failed.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    for collection in disco.collections() {
        info!("Repairing collection {}.", collection.id());
        repair_collection(storage.as_ref(), collection.href(), failed.clone()).await;
    }
}

// This function keeps a list of "seen" UIDs; implementing concurrency should be done with care. We
// also have have N workers fetching other collections for the same storage, so using multiple
// workers here can compound multiplicatively.
async fn repair_collection(storage: &dyn Storage, collection: &str, failed: Arc<AtomicU32>) {
    let all_fetched = match storage.get_all_items(collection).await {
        Ok(ok) => ok,
        Err(err) => {
            error!("Could not fetch all items for {collection}: {err}.");
            failed.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let mut all_uids: HashSet<_> = all_fetched.iter().filter_map(|f| f.item.uid()).collect();

    for fetched in &all_fetched {
        if fetched.item.uid().is_none() {
            info!("Item has no UID; will repair it");
            let mut uid = fetched.item.hash().to_string();
            if all_uids.contains(&uid) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Current time should be after Unix epoch")
                    .as_nanos()
                    .to_string();
                uid.push('.');
                uid.push_str(&now);
            }
            if all_uids.contains(&uid) {
                // Should never really happen.
                error!("Failed to produce a unique UID for item!");
                failed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            all_uids.insert(uid.clone());

            let with_uid = fetched.item.with_uid(&uid);
            if let Err(err) = storage
                .update_item(&fetched.href, &fetched.etag, &with_uid)
                .await
            {
                error!("Failed to update {}: {}.", fetched.href, err);
                failed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            info!("Updated UID for {}.", fetched.href);
        }
    }
}
