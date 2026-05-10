#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::collections::HashMap;

use async_trait::async_trait;
use idb::{Database, DatabaseEvent, Factory, IndexParams, KeyPath, ObjectStoreParams, TransactionMode};
use rouchdb_core::adapter::Adapter;
use rouchdb_core::document::*;
use rouchdb_core::error::{Result, RouchError};
use rouchdb_core::merge::{collect_conflicts, is_deleted, merge_tree, winning_rev};
use rouchdb_core::rev_tree::{
    NodeOpts, RevPath, RevStatus, RevTree, build_path_from_revs, collect_leaves,
    find_rev_ancestry, rev_exists,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wasm_bindgen::JsValue;

use crate::util::*;

const DEFAULT_REV_LIMIT: u64 = 1000;

// ---------------------------------------------------------------------------
// Stored types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct StoredDoc {
    id: String,
    rev_tree: String, // JSON-encoded RevTree
    seq: u64,
}

/// Composite key "<doc_id>\x00<rev>" stored in the `key` field so IDB can
/// use it as the object store key path. The `key` field is required by the
/// IDB key path configured in `on_upgrade_needed`.
#[derive(Serialize, Deserialize)]
struct StoredRev {
    key: String, // "<doc_id>\x00<rev>"
    doc_id: String,
    rev: String,
    data: String,    // JSON-encoded serde_json::Value
    deleted: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredChange {
    seq: u64,
    doc_id: String,
    deleted: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredLocalDoc {
    id: String,
    data: String, // JSON-encoded serde_json::Value
}

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

pub struct IndexedDbAdapter {
    db: Database,
    name: String,
    update_seq: Cell<u64>,
}

impl IndexedDbAdapter {
    pub async fn open(name: &str) -> Result<Self> {
        let factory = Factory::new().map_err(idb_err)?;
        let mut open_req = factory.open(name, Some(1)).map_err(idb_err)?;

        open_req.on_upgrade_needed(|evt| {
            let db = evt.database().expect("db from upgrade event");

            // docs: keyed by id
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("id")));
            db.create_object_store("docs", p).ok();

            // revs: keyed by composite "key" field
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("key")));
            if let Ok(store) = db.create_object_store("revs", p) {
                let mut idx = IndexParams::new();
                idx.unique(false);
                store.create_index("by_doc_id", KeyPath::new_single("doc_id"), Some(idx)).ok();
            }

            // changes: keyed by seq (u64)
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("seq")));
            if let Ok(store) = db.create_object_store("changes", p) {
                let mut idx = IndexParams::new();
                idx.unique(true);
                store.create_index("by_doc_id", KeyPath::new_single("doc_id"), Some(idx)).ok();
            }

            // local_docs: keyed by id
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("id")));
            db.create_object_store("local_docs", p).ok();

            // attachments: keyed by digest
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("digest")));
            db.create_object_store("attachments", p).ok();

            // meta: keyed by key
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("key")));
            db.create_object_store("meta", p).ok();
        });

        let db = open_req.await.map_err(idb_err)?;
        let update_seq = load_update_seq(&db).await.unwrap_or(0);

        Ok(Self {
            db,
            name: name.to_string(),
            update_seq: Cell::new(update_seq),
        })
    }
}

// ---------------------------------------------------------------------------
// Single-request helpers (each opens its own read-only tx)
// ---------------------------------------------------------------------------

async fn load_update_seq(db: &Database) -> Result<u64> {
    let tx = db.transaction(&["meta"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let store = tx.object_store("meta").map_err(idb_err)?;
    let key = JsValue::from_str("update_seq");
    match store.get(key).map_err(idb_err)?.await.map_err(idb_err)? {
        None => Ok(0),
        Some(v) => {
            let obj: serde_json::Value = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
            Ok(obj["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0))
        }
    }
}

async fn load_doc(db: &Database, id: &str) -> Result<Option<(RevTree, u64)>> {
    let tx = db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let store = tx.object_store("docs").map_err(idb_err)?;
    match store.get(JsValue::from_str(id)).map_err(idb_err)?.await.map_err(idb_err)? {
        None => Ok(None),
        Some(v) => {
            let sd: StoredDoc = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
            let tree: RevTree = serde_json::from_str(&sd.rev_tree).map_err(serde_err)?;
            Ok(Some((tree, sd.seq)))
        }
    }
}

async fn load_rev_data(db: &Database, doc_id: &str, rev: &str) -> Result<Option<(serde_json::Value, bool)>> {
    let key_str = format!("{}\x00{}", doc_id, rev);
    let tx = db.transaction(&["revs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let store = tx.object_store("revs").map_err(idb_err)?;
    match store.get(JsValue::from_str(&key_str)).map_err(idb_err)?.await.map_err(idb_err)? {
        None => Ok(None),
        Some(v) => {
            let sr: StoredRev = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
            let data: serde_json::Value = serde_json::from_str(&sr.data).map_err(serde_err)?;
            Ok(Some((data, sr.deleted)))
        }
    }
}

// ---------------------------------------------------------------------------
// Multi-request batch read helper
// All get() requests are queued synchronously before any await so the
// transaction doesn't auto-commit between requests.
// ---------------------------------------------------------------------------

/// For each (doc_id, rev) pair, fetches rev body data in a single transaction.
/// All requests are submitted synchronously; only then are they awaited.
async fn batch_load_rev_data(
    db: &Database,
    keys: &[(String, String)], // (doc_id, rev)
) -> Result<Vec<Option<(serde_json::Value, bool)>>> {
    if keys.is_empty() {
        return Ok(vec![]);
    }
    let tx = db.transaction(&["revs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let store = tx.object_store("revs").map_err(idb_err)?;

    // Queue all requests synchronously
    let requests: Vec<_> = keys
        .iter()
        .map(|(doc_id, rev)| {
            let k = JsValue::from_str(&format!("{}\x00{}", doc_id, rev));
            store.get(k).map_err(idb_err)
        })
        .collect::<Result<Vec<_>>>()?;

    // Await all
    let mut results = Vec::with_capacity(requests.len());
    for req in requests {
        let val = req.await.map_err(idb_err)?;
        let parsed = val.map(|v| -> Result<(serde_json::Value, bool)> {
            let sr: StoredRev = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
            let data: serde_json::Value = serde_json::from_str(&sr.data).map_err(serde_err)?;
            Ok((data, sr.deleted))
        }).transpose()?;
        results.push(parsed);
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Atomic write helper — all store operations queued before any await
// ---------------------------------------------------------------------------

impl IndexedDbAdapter {
    async fn write_doc(
        &self,
        doc_id: &str,
        tree: &RevTree,
        new_seq: u64,
        old_seq: Option<u64>,
        rev_str: &str,
        doc: &Document,
    ) -> Result<()> {
        // Serialize everything before opening the transaction
        let tree_json = serde_json::to_string(tree).map_err(serde_err)?;
        let data_json = serde_json::to_string(&doc.data).map_err(serde_err)?;
        let is_doc_deleted = is_deleted(tree);

        let sd_js = serde_wasm_bindgen::to_value(&StoredDoc {
            id: doc_id.to_string(),
            rev_tree: tree_json,
            seq: new_seq,
        }).map_err(serde_err)?;

        let rev_key = format!("{}\x00{}", doc_id, rev_str);
        let sr_js = serde_wasm_bindgen::to_value(&StoredRev {
            key: rev_key,
            doc_id: doc_id.to_string(),
            rev: rev_str.to_string(),
            data: data_json,
            deleted: doc.deleted,
        }).map_err(serde_err)?;

        let change_js = serde_wasm_bindgen::to_value(&StoredChange {
            seq: new_seq,
            doc_id: doc_id.to_string(),
            deleted: is_doc_deleted,
        }).map_err(serde_err)?;

        let meta_js = serde_wasm_bindgen::to_value(
            &serde_json::json!({ "key": "update_seq", "value": new_seq.to_string() })
        ).map_err(serde_err)?;

        // Open one read-write transaction
        let tx = self
            .db
            .transaction(&["docs", "revs", "changes", "meta"], TransactionMode::ReadWrite)
            .map_err(idb_err)?;

        let docs_store = tx.object_store("docs").map_err(idb_err)?;
        let revs_store = tx.object_store("revs").map_err(idb_err)?;
        let changes_store = tx.object_store("changes").map_err(idb_err)?;
        let meta_store = tx.object_store("meta").map_err(idb_err)?;

        // Queue ALL requests synchronously before any await
        let r_doc = docs_store.put(&sd_js, None).map_err(idb_err)?;
        let r_rev = revs_store.put(&sr_js, None).map_err(idb_err)?;
        let r_del = if let Some(old_s) = old_seq {
            let k = serde_wasm_bindgen::to_value(&old_s).map_err(serde_err)?;
            Some(changes_store.delete(k).map_err(idb_err)?)
        } else {
            None
        };
        let r_change = changes_store.put(&change_js, None).map_err(idb_err)?;
        let r_meta = meta_store.put(&meta_js, None).map_err(idb_err)?;

        // Await all results
        r_doc.await.map_err(idb_err)?;
        r_rev.await.map_err(idb_err)?;
        if let Some(r) = r_del {
            r.await.map_err(idb_err)?;
        }
        r_change.await.map_err(idb_err)?;
        r_meta.await.map_err(idb_err)?;
        tx.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Adapter implementation
// ---------------------------------------------------------------------------

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Adapter for IndexedDbAdapter {
    async fn info(&self) -> Result<DbInfo> {
        let tx = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
        let store = tx.object_store("docs").map_err(idb_err)?;
        let all = store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?;

        let doc_count = all
            .into_iter()
            .filter(|v| {
                serde_wasm_bindgen::from_value::<StoredDoc>(v.clone())
                    .ok()
                    .and_then(|sd| serde_json::from_str::<RevTree>(&sd.rev_tree).ok())
                    .map(|tree| !is_deleted(&tree))
                    .unwrap_or(false)
            })
            .count() as u64;

        Ok(DbInfo {
            db_name: self.name.clone(),
            doc_count,
            update_seq: Seq::Num(self.update_seq.get()),
        })
    }

    async fn get_local(&self, id: &str) -> Result<serde_json::Value> {
        let tx = self.db.transaction(&["local_docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
        let store = tx.object_store("local_docs").map_err(idb_err)?;
        match store.get(JsValue::from_str(id)).map_err(idb_err)?.await.map_err(idb_err)? {
            None => Err(RouchError::NotFound(format!("_local/{}", id))),
            Some(v) => {
                let sd: StoredLocalDoc = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
                let data: serde_json::Value = serde_json::from_str(&sd.data).map_err(serde_err)?;
                Ok(data)
            }
        }
    }

    async fn put_local(&self, id: &str, doc: serde_json::Value) -> Result<()> {
        let sd = StoredLocalDoc {
            id: id.to_string(),
            data: serde_json::to_string(&doc).map_err(serde_err)?,
        };
        let js_val = serde_wasm_bindgen::to_value(&sd).map_err(serde_err)?;
        let tx = self.db.transaction(&["local_docs"], TransactionMode::ReadWrite).map_err(idb_err)?;
        let store = tx.object_store("local_docs").map_err(idb_err)?;
        let r = store.put(&js_val, None).map_err(idb_err)?;
        r.await.map_err(idb_err)?;
        tx.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        Ok(())
    }

    async fn remove_local(&self, id: &str) -> Result<()> {
        // Check existence first (separate read-only tx)
        {
            let tx = self.db.transaction(&["local_docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
            let store = tx.object_store("local_docs").map_err(idb_err)?;
            let val = store.get(JsValue::from_str(id)).map_err(idb_err)?.await.map_err(idb_err)?;
            if val.is_none() {
                return Err(RouchError::NotFound(format!("_local/{}", id)));
            }
        }
        let tx = self.db.transaction(&["local_docs"], TransactionMode::ReadWrite).map_err(idb_err)?;
        let store = tx.object_store("local_docs").map_err(idb_err)?;
        let r = store.delete(JsValue::from_str(id)).map_err(idb_err)?;
        r.await.map_err(idb_err)?;
        tx.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        Ok(())
    }

    async fn bulk_docs(&self, docs: Vec<Document>, opts: BulkDocsOptions) -> Result<Vec<DocResult>> {
        let mut results = Vec::with_capacity(docs.len());
        for doc in docs {
            let r = if opts.new_edits {
                self.process_new_edits(doc).await
            } else {
                self.process_replication(doc).await
            };
            results.push(r);
        }
        Ok(results)
    }

    async fn get(&self, id: &str, opts: GetOptions) -> Result<Document> {
        let (tree, _seq) = load_doc(&self.db, id)
            .await?
            .ok_or_else(|| RouchError::NotFound(id.to_string()))?;

        let mut target_rev = if let Some(ref rev_str) = opts.rev {
            rev_str.clone()
        } else {
            winning_rev(&tree)
                .ok_or_else(|| RouchError::NotFound(id.to_string()))?
                .to_string()
        };

        if opts.latest && opts.rev.is_some() {
            let leaves = collect_leaves(&tree);
            if !leaves.iter().any(|l| l.rev_string() == target_rev) {
                if let Some(leaf) = leaves.first() {
                    target_rev = leaf.rev_string();
                }
            }
        }

        let (data, deleted) = load_rev_data(&self.db, id, &target_rev)
            .await?
            .unwrap_or((serde_json::Value::Object(serde_json::Map::new()), false));

        if deleted && opts.rev.is_none() {
            return Err(RouchError::NotFound(id.to_string()));
        }

        let (pos, hash) = parse_rev(&target_rev)?;
        let mut doc = Document {
            id: id.to_string(),
            rev: Some(Revision::new(pos, hash)),
            deleted,
            data,
            attachments: HashMap::new(),
        };

        if opts.conflicts {
            let conflicts = collect_conflicts(&tree);
            if !conflicts.is_empty() {
                if let serde_json::Value::Object(ref mut map) = doc.data {
                    map.insert(
                        "_conflicts".to_string(),
                        serde_json::Value::Array(
                            conflicts.iter().map(|c| serde_json::Value::String(c.to_string())).collect(),
                        ),
                    );
                }
            }
        }

        Ok(doc)
    }

    async fn all_docs(&self, opts: AllDocsOptions) -> Result<AllDocsResponse> {
        // Phase 1: load all docs from the docs store
        let all_stored: Vec<StoredDoc> = {
            let tx = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
            let store = tx.object_store("docs").map_err(idb_err)?;
            store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?
                .into_iter()
                .filter_map(|v| serde_wasm_bindgen::from_value(v).ok())
                .collect()
        };

        // Build sorted list of IDs
        let mut doc_ids: Vec<String> = all_stored.iter().map(|sd| sd.id.clone()).collect();
        doc_ids.sort();
        if opts.descending {
            doc_ids.reverse();
        }

        // Build a lookup map for StoredDoc
        let doc_map: HashMap<String, StoredDoc> = all_stored.into_iter().map(|sd| (sd.id.clone(), sd)).collect();

        let target_keys: Vec<String> = if let Some(ref keys) = opts.keys {
            keys.clone()
        } else if let Some(ref key) = opts.key {
            vec![key.clone()]
        } else {
            doc_ids
        };

        // Filter keys and build candidate list
        struct Candidate {
            id: String,
            tree: RevTree,
        }

        let mut candidates: Vec<Candidate> = Vec::new();
        for key in &target_keys {
            if opts.keys.is_none() && opts.key.is_none() {
                if let Some(ref start) = opts.start_key {
                    if (!opts.descending && key.as_str() < start.as_str())
                        || (opts.descending && key.as_str() > start.as_str())
                    {
                        continue;
                    }
                }
                if let Some(ref end) = opts.end_key {
                    let past = if opts.inclusive_end {
                        (!opts.descending && key.as_str() > end.as_str())
                            || (opts.descending && key.as_str() < end.as_str())
                    } else {
                        (!opts.descending && key.as_str() >= end.as_str())
                            || (opts.descending && key.as_str() <= end.as_str())
                    };
                    if past {
                        continue;
                    }
                }
            }
            if let Some(sd) = doc_map.get(key.as_str()) {
                if let Ok(tree) = serde_json::from_str::<RevTree>(&sd.rev_tree) {
                    candidates.push(Candidate { id: key.clone(), tree });
                }
            }
        }

        // Phase 2: batch-load rev data for include_docs candidates
        let rev_keys: Vec<(String, String)> = if opts.include_docs {
            candidates.iter().filter_map(|c| {
                let deleted = is_deleted(&c.tree);
                if deleted {
                    return None;
                }
                let winner = winning_rev(&c.tree)?;
                Some((c.id.clone(), winner.to_string()))
            }).collect()
        } else {
            vec![]
        };

        let rev_data = batch_load_rev_data(&self.db, &rev_keys).await?;
        // Build rev lookup: doc_id -> (data, deleted)
        let rev_lookup: HashMap<String, (serde_json::Value, bool)> = rev_keys
            .into_iter()
            .zip(rev_data.into_iter())
            .filter_map(|((doc_id, _rev), data)| Some((doc_id, data?)))
            .collect();

        // Build rows
        let mut rows: Vec<AllDocsRow> = Vec::new();
        for c in &candidates {
            let winner = match winning_rev(&c.tree) {
                Some(w) => w,
                None => continue,
            };
            let deleted = is_deleted(&c.tree);
            if deleted && opts.keys.is_none() {
                continue;
            }

            let doc_json = if opts.include_docs && !deleted {
                rev_lookup.get(&c.id).map(|(data, _)| {
                    let mut obj = match data {
                        serde_json::Value::Object(m) => m.clone(),
                        _ => serde_json::Map::new(),
                    };
                    obj.insert("_id".into(), serde_json::Value::String(c.id.clone()));
                    obj.insert("_rev".into(), serde_json::Value::String(winner.to_string()));
                    if opts.conflicts {
                        let conflicts = collect_conflicts(&c.tree);
                        if !conflicts.is_empty() {
                            obj.insert(
                                "_conflicts".into(),
                                serde_json::Value::Array(
                                    conflicts.iter().map(|x| serde_json::Value::String(x.to_string())).collect(),
                                ),
                            );
                        }
                    }
                    serde_json::Value::Object(obj)
                })
            } else {
                None
            };

            rows.push(AllDocsRow {
                id: c.id.clone(),
                key: c.id.clone(),
                value: AllDocsRowValue {
                    rev: winner.to_string(),
                    deleted: if deleted { Some(true) } else { None },
                },
                doc: doc_json,
            });
        }

        let total_rows = rows.len() as u64;
        let skip = opts.skip as usize;
        if skip > 0 {
            rows = rows.into_iter().skip(skip).collect();
        }
        if let Some(limit) = opts.limit {
            rows.truncate(limit as usize);
        }

        Ok(AllDocsResponse {
            total_rows,
            offset: opts.skip,
            rows,
            update_seq: if opts.update_seq { Some(Seq::Num(self.update_seq.get())) } else { None },
        })
    }

    async fn changes(&self, opts: ChangesOptions) -> Result<ChangesResponse> {
        // Phase 1: load all changes records
        let all_changes: Vec<StoredChange> = {
            let tx = self.db.transaction(&["changes"], TransactionMode::ReadOnly).map_err(idb_err)?;
            let store = tx.object_store("changes").map_err(idb_err)?;
            store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?
                .into_iter()
                .filter_map(|v| serde_wasm_bindgen::from_value(v).ok())
                .collect()
        };

        let mut entries: Vec<StoredChange> = all_changes
            .into_iter()
            .filter(|c| c.seq > opts.since.as_num())
            .collect();

        entries.sort_by_key(|c| c.seq);
        if opts.descending {
            entries.reverse();
        }

        // Apply doc_ids filter and limit
        let filtered: Vec<StoredChange> = entries.into_iter()
            .filter(|c| {
                opts.doc_ids.as_ref().map(|ids| ids.contains(&c.doc_id)).unwrap_or(true)
            })
            .take(opts.limit.map(|l| l as usize).unwrap_or(usize::MAX))
            .collect();

        if filtered.is_empty() {
            return Ok(ChangesResponse { results: vec![], last_seq: opts.since });
        }

        // Phase 2: batch-load the winning rev for each changed doc
        let doc_ids: Vec<String> = filtered.iter().map(|c| c.doc_id.clone()).collect();
        let trees: Vec<Option<RevTree>> = {
            let tx = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
            let store = tx.object_store("docs").map_err(idb_err)?;
            // Queue all get requests synchronously
            let reqs: Vec<_> = doc_ids.iter()
                .map(|id| store.get(JsValue::from_str(id)).map_err(idb_err))
                .collect::<Result<Vec<_>>>()?;
            let mut out = Vec::with_capacity(reqs.len());
            for r in reqs {
                let val = r.await.map_err(idb_err)?;
                let tree = val.and_then(|v| {
                    serde_wasm_bindgen::from_value::<StoredDoc>(v).ok()
                        .and_then(|sd| serde_json::from_str::<RevTree>(&sd.rev_tree).ok())
                });
                out.push(tree);
            }
            out
        };

        // Phase 3: if include_docs, batch-load rev data
        let rev_keys: Vec<(String, String)> = if opts.include_docs {
            filtered.iter().zip(trees.iter()).filter_map(|(c, tree_opt)| {
                let tree = tree_opt.as_ref()?;
                let winner = winning_rev(tree)?;
                Some((c.doc_id.clone(), winner.to_string()))
            }).collect()
        } else {
            vec![]
        };
        let rev_data = batch_load_rev_data(&self.db, &rev_keys).await?;
        let rev_lookup: HashMap<String, (serde_json::Value, bool)> = rev_keys.into_iter()
            .zip(rev_data.into_iter())
            .filter_map(|((doc_id, _), data)| Some((doc_id, data?)))
            .collect();

        // Build results
        let mut results = Vec::new();
        for (c, tree_opt) in filtered.iter().zip(trees.iter()) {
            let rev_str = tree_opt.as_ref()
                .and_then(|t| winning_rev(t))
                .map(|r| r.to_string())
                .unwrap_or_default();

            let changes_list = if opts.style == ChangesStyle::AllDocs {
                tree_opt.as_ref()
                    .map(|t| collect_leaves(t).iter().map(|l| ChangeRev { rev: l.rev_string() }).collect())
                    .unwrap_or_else(|| vec![ChangeRev { rev: rev_str.clone() }])
            } else {
                vec![ChangeRev { rev: rev_str.clone() }]
            };

            let doc = if opts.include_docs {
                rev_lookup.get(&c.doc_id).map(|(data, _)| {
                    let mut obj = match data {
                        serde_json::Value::Object(m) => m.clone(),
                        _ => serde_json::Map::new(),
                    };
                    obj.insert("_id".into(), serde_json::Value::String(c.doc_id.clone()));
                    obj.insert("_rev".into(), serde_json::Value::String(rev_str.clone()));
                    if c.deleted {
                        obj.insert("_deleted".into(), serde_json::Value::Bool(true));
                    }
                    serde_json::Value::Object(obj)
                })
            } else {
                None
            };

            results.push(ChangeEvent {
                seq: Seq::Num(c.seq),
                id: c.doc_id.clone(),
                changes: changes_list,
                deleted: c.deleted,
                doc,
                conflicts: None,
            });
        }

        let last_seq = results.last().map(|r| r.seq.clone()).unwrap_or(opts.since);
        Ok(ChangesResponse { results, last_seq })
    }

    async fn revs_diff(&self, revs: HashMap<String, Vec<String>>) -> Result<RevsDiffResponse> {
        let mut results = HashMap::new();
        for (doc_id, rev_list) in revs {
            let tree_opt = load_doc(&self.db, &doc_id).await?.map(|(t, _)| t);
            let mut missing = Vec::new();
            let mut possible_ancestors = Vec::new();
            for rev_str in &rev_list {
                let (pos, hash) = parse_rev(rev_str)?;
                let exists = tree_opt.as_ref().map(|t| rev_exists(t, pos, &hash)).unwrap_or(false);
                if !exists {
                    missing.push(rev_str.clone());
                    if let Some(ref tree) = tree_opt {
                        for leaf in collect_leaves(tree) {
                            if leaf.pos < pos {
                                possible_ancestors.push(leaf.rev_string());
                            }
                        }
                    }
                }
            }
            if !missing.is_empty() {
                results.insert(doc_id, RevsDiffResult { missing, possible_ancestors });
            }
        }
        Ok(RevsDiffResponse { results })
    }

    async fn bulk_get(&self, docs: Vec<BulkGetItem>) -> Result<BulkGetResponse> {
        let mut results = Vec::new();
        for item in docs {
            let mut bulk_docs = Vec::new();
            match load_doc(&self.db, &item.id).await? {
                None => {
                    bulk_docs.push(BulkGetDoc {
                        ok: None,
                        error: Some(BulkGetError {
                            id: item.id.clone(),
                            rev: item.rev.clone().unwrap_or_default(),
                            error: "not_found".into(),
                            reason: "missing".into(),
                        }),
                    });
                }
                Some((tree, _)) => {
                    let rev_str = if let Some(ref r) = item.rev {
                        r.clone()
                    } else {
                        match winning_rev(&tree) {
                            Some(w) => w.to_string(),
                            None => {
                                bulk_docs.push(BulkGetDoc {
                                    ok: None,
                                    error: Some(BulkGetError {
                                        id: item.id.clone(),
                                        rev: item.rev.clone().unwrap_or_default(),
                                        error: "not_found".into(),
                                        reason: "missing".into(),
                                    }),
                                });
                                results.push(BulkGetResult { id: item.id, docs: bulk_docs });
                                continue;
                            }
                        }
                    };

                    match load_rev_data(&self.db, &item.id, &rev_str).await? {
                        None => {
                            bulk_docs.push(BulkGetDoc {
                                ok: None,
                                error: Some(BulkGetError {
                                    id: item.id.clone(),
                                    rev: rev_str,
                                    error: "not_found".into(),
                                    reason: "missing".into(),
                                }),
                            });
                        }
                        Some((data, deleted)) => {
                            let mut obj = match data {
                                serde_json::Value::Object(m) => m,
                                _ => serde_json::Map::new(),
                            };
                            obj.insert("_id".into(), serde_json::Value::String(item.id.clone()));
                            obj.insert("_rev".into(), serde_json::Value::String(rev_str.clone()));
                            if deleted {
                                obj.insert("_deleted".into(), serde_json::Value::Bool(true));
                            }
                            if let Ok((pos, ref hash)) = parse_rev(&rev_str) {
                                if let Some(ancestry) = find_rev_ancestry(&tree, pos, hash) {
                                    obj.insert(
                                        "_revisions".into(),
                                        serde_json::json!({ "start": pos, "ids": ancestry }),
                                    );
                                }
                            }
                            bulk_docs.push(BulkGetDoc { ok: Some(serde_json::Value::Object(obj)), error: None });
                        }
                    }
                }
            }
            results.push(BulkGetResult { id: item.id, docs: bulk_docs });
        }
        Ok(BulkGetResponse { results })
    }

    async fn put_attachment(
        &self,
        doc_id: &str,
        att_id: &str,
        rev: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<DocResult> {
        let digest = compute_attachment_digest(&data);
        let length = data.len() as u64;

        // Store attachment bytes
        {
            let att_js = serde_wasm_bindgen::to_value(&serde_json::json!({
                "digest": digest,
                "content_type": content_type,
                "data": data,
            })).map_err(serde_err)?;
            let tx = self.db.transaction(&["attachments"], TransactionMode::ReadWrite).map_err(idb_err)?;
            let store = tx.object_store("attachments").map_err(idb_err)?;
            store.put(&att_js, None).map_err(idb_err)?.await.map_err(idb_err)?;
            tx.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        }

        let (tree, _) = load_doc(&self.db, doc_id)
            .await?
            .ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
        let winner = winning_rev(&tree).ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
        if winner.to_string() != rev {
            return Err(RouchError::Conflict);
        }

        let (doc_data, _) = load_rev_data(&self.db, doc_id, rev)
            .await?
            .unwrap_or((serde_json::Value::Object(serde_json::Map::new()), false));

        let doc = Document {
            id: doc_id.to_string(),
            rev: Some(winner),
            deleted: false,
            data: doc_data,
            attachments: {
                let mut m = HashMap::new();
                m.insert(att_id.to_string(), AttachmentMeta {
                    content_type: content_type.to_string(),
                    digest,
                    length,
                    stub: true,
                    data: None,
                });
                m
            },
        };
        Ok(self.process_new_edits(doc).await)
    }

    async fn get_attachment(
        &self,
        doc_id: &str,
        att_id: &str,
        opts: GetAttachmentOptions,
    ) -> Result<Vec<u8>> {
        let rev_str = if let Some(ref r) = opts.rev {
            r.clone()
        } else {
            let (tree, _) = load_doc(&self.db, doc_id)
                .await?
                .ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
            winning_rev(&tree)
                .ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?
                .to_string()
        };

        let (data, _) = load_rev_data(&self.db, doc_id, &rev_str)
            .await?
            .ok_or_else(|| RouchError::NotFound(format!("{}/{}", doc_id, att_id)))?;

        let digest = data
            .get("_attachments").and_then(|a| a.get(att_id))
            .and_then(|m| m.get("digest")).and_then(|d| d.as_str())
            .map(String::from)
            .ok_or_else(|| RouchError::NotFound(format!("{}/{}", doc_id, att_id)))?;

        let tx = self.db.transaction(&["attachments"], TransactionMode::ReadOnly).map_err(idb_err)?;
        let store = tx.object_store("attachments").map_err(idb_err)?;
        match store.get(JsValue::from_str(&digest)).map_err(idb_err)?.await.map_err(idb_err)? {
            None => Err(RouchError::NotFound(format!("{}/{}", doc_id, att_id))),
            Some(v) => {
                let record: serde_json::Value = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
                let bytes: Vec<u8> = serde_json::from_value(record["data"].clone()).map_err(serde_err)?;
                Ok(bytes)
            }
        }
    }

    async fn remove_attachment(&self, doc_id: &str, att_id: &str, rev: &str) -> Result<DocResult> {
        let _ = att_id;
        let (tree, _) = load_doc(&self.db, doc_id)
            .await?
            .ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
        let winner = winning_rev(&tree).ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
        if winner.to_string() != rev {
            return Err(RouchError::Conflict);
        }
        let (doc_data, _) = load_rev_data(&self.db, doc_id, rev)
            .await?
            .unwrap_or((serde_json::Value::Object(serde_json::Map::new()), false));

        let doc = Document {
            id: doc_id.to_string(),
            rev: Some(winner),
            deleted: false,
            data: doc_data,
            attachments: HashMap::new(),
        };
        Ok(self.process_new_edits(doc).await)
    }

    async fn compact(&self) -> Result<()> {
        // Phase 1: load all docs (read-only tx)
        let all_docs: Vec<StoredDoc> = {
            let tx = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
            let store = tx.object_store("docs").map_err(idb_err)?;
            store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?
                .into_iter()
                .filter_map(|v| serde_wasm_bindgen::from_value(v).ok())
                .collect()
        };

        // Compute leaf sets per doc
        let mut leaf_map: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for sd in &all_docs {
            if let Ok(tree) = serde_json::from_str::<RevTree>(&sd.rev_tree) {
                let leaves = collect_leaves(&tree).iter().map(|l| l.rev_string()).collect();
                leaf_map.insert(sd.id.clone(), leaves);
            }
        }

        // Phase 2: load all revs (read-only tx)
        let non_leaf_keys: Vec<String> = {
            let tx = self.db.transaction(&["revs"], TransactionMode::ReadOnly).map_err(idb_err)?;
            let store = tx.object_store("revs").map_err(idb_err)?;
            let all_revs: Vec<StoredRev> = store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?
                .into_iter()
                .filter_map(|v| serde_wasm_bindgen::from_value(v).ok())
                .collect();
            all_revs.into_iter()
                .filter(|sr| {
                    !leaf_map.get(&sr.doc_id).map(|ls| ls.contains(&sr.rev)).unwrap_or(false)
                })
                .map(|sr| sr.key)
                .collect()
        };

        if non_leaf_keys.is_empty() {
            return Ok(());
        }

        // Phase 3: delete non-leaf revs in a single read-write transaction
        let tx = self.db.transaction(&["revs"], TransactionMode::ReadWrite).map_err(idb_err)?;
        let store = tx.object_store("revs").map_err(idb_err)?;
        // Queue all delete requests synchronously
        let del_reqs: Vec<_> = non_leaf_keys.iter()
            .map(|k| store.delete(JsValue::from_str(k)).map_err(idb_err))
            .collect::<Result<Vec<_>>>()?;
        for r in del_reqs {
            r.await.map_err(idb_err)?;
        }
        tx.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        Ok(())
    }

    async fn destroy(&self) -> Result<()> {
        for name in &["docs", "revs", "changes", "local_docs", "attachments", "meta"] {
            let tx = self.db.transaction(&[name], TransactionMode::ReadWrite).map_err(idb_err)?;
            let store = tx.object_store(name).map_err(idb_err)?;
            store.clear().map_err(idb_err)?.await.map_err(idb_err)?;
            tx.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        }
        self.update_seq.set(0);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Write processing helpers
// ---------------------------------------------------------------------------

impl IndexedDbAdapter {
    async fn process_new_edits(&self, doc: Document) -> DocResult {
        let doc_id = if doc.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            doc.id.clone()
        };

        let existing = match load_doc(&self.db, &doc_id).await {
            Ok(v) => v,
            Err(e) => return db_err(&doc_id, e),
        };

        if let Some((ref tree, _)) = existing {
            let winner = winning_rev(tree);
            match (&doc.rev, &winner) {
                (Some(prov), Some(curr)) => {
                    if prov.to_string() != curr.to_string() {
                        return conflict_result(doc_id);
                    }
                }
                (None, Some(_)) if !is_deleted(tree) => return conflict_result(doc_id),
                _ => {}
            }
        } else if doc.rev.is_some() {
            return DocResult {
                ok: false,
                id: doc_id,
                rev: None,
                error: Some("not_found".into()),
                reason: Some("missing".into()),
            };
        }

        let new_pos = doc.rev.as_ref().map(|r| r.pos + 1).unwrap_or(1);
        let prev_rev_str = doc.rev.as_ref().map(|r| r.to_string());
        let new_hash = generate_rev_hash(&doc.data, doc.deleted, prev_rev_str.as_deref());
        let new_rev_str = rev_string(new_pos, &new_hash);

        let mut rev_hashes = vec![new_hash.clone()];
        if let Some(ref prev) = doc.rev {
            rev_hashes.push(prev.hash.clone());
        }

        let new_path = build_path_from_revs(
            new_pos,
            &rev_hashes,
            NodeOpts { deleted: doc.deleted },
            RevStatus::Available,
        );

        let existing_tree = existing.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
        let existing_seq = existing.as_ref().map(|(_, s)| *s);
        let (merged_tree, _) = merge_tree(&existing_tree, &new_path, DEFAULT_REV_LIMIT);

        let new_seq = self.update_seq.get() + 1;
        self.update_seq.set(new_seq);

        if let Err(e) = self.write_doc(&doc_id, &merged_tree, new_seq, existing_seq, &new_rev_str, &doc).await {
            return db_err(&doc_id, e);
        }

        DocResult { ok: true, id: doc_id, rev: Some(new_rev_str), error: None, reason: None }
    }

    async fn process_replication(&self, mut doc: Document) -> DocResult {
        let doc_id = doc.id.clone();
        let rev = match doc.rev.clone() {
            Some(r) => r,
            None => return DocResult {
                ok: false, id: doc_id, rev: None,
                error: Some("bad_request".into()), reason: Some("missing _rev".into()),
            },
        };

        let rev_str = rev.to_string();

        let new_path = if let Some(revisions) = doc.data.get("_revisions") {
            let start = revisions["start"].as_u64().unwrap_or(rev.pos);
            let ids: Vec<String> = revisions["ids"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_else(|| vec![rev.hash.clone()]);
            build_path_from_revs(start, &ids, NodeOpts { deleted: doc.deleted }, RevStatus::Available)
        } else {
            RevPath {
                pos: rev.pos,
                tree: rouchdb_core::rev_tree::RevNode {
                    hash: rev.hash.clone(),
                    status: RevStatus::Available,
                    opts: NodeOpts { deleted: doc.deleted },
                    children: vec![],
                },
            }
        };

        if let serde_json::Value::Object(ref mut map) = doc.data {
            map.remove("_revisions");
        }

        let existing = match load_doc(&self.db, &doc_id).await {
            Ok(v) => v,
            Err(e) => return db_err(&doc_id, e),
        };

        let existing_tree = existing.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
        let existing_seq = existing.as_ref().map(|(_, s)| *s);
        let (merged_tree, _) = merge_tree(&existing_tree, &new_path, DEFAULT_REV_LIMIT);

        let new_seq = self.update_seq.get() + 1;
        self.update_seq.set(new_seq);

        if let Err(e) = self.write_doc(&doc_id, &merged_tree, new_seq, existing_seq, &rev_str, &doc).await {
            return db_err(&doc_id, e);
        }

        DocResult { ok: true, id: doc_id, rev: Some(rev_str), error: None, reason: None }
    }
}

fn conflict_result(id: String) -> DocResult {
    DocResult {
        ok: false, id, rev: None,
        error: Some("conflict".into()),
        reason: Some("Document update conflict".into()),
    }
}

fn db_err(id: &str, e: RouchError) -> DocResult {
    DocResult {
        ok: false,
        id: id.to_string(),
        rev: None,
        error: Some("database_error".into()),
        reason: Some(format!("{:?}", e)),
    }
}
