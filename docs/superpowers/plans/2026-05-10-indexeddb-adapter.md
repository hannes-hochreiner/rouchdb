# IndexedDB Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rouchdb-adapter-indexeddb`, a WASM-only `Adapter` implementation backed by browser IndexedDB, enabling Rust WASM apps (Leptos, Yew) to use RouchDB with persistent local storage and full CouchDB bidirectional sync.

**Architecture:** Modify `rouchdb-core`'s `Adapter` trait to be `?Send`-compatible on WASM via `MaybeSend`/`MaybeSync` marker traits; make `rouchdb-replication` and `rouchdb-adapter-http` WASM-compatible; implement `IndexedDbAdapter` using six normalized IndexedDB object stores (docs, revs, changes, local\_docs, attachments, meta).

**Tech Stack:** Rust / `wasm32-unknown-unknown`, `idb` 0.6 (IndexedDB wrapper), `serde-wasm-bindgen` 0.6, `wasm-bindgen`, `wasm-bindgen-futures`, `wasm-pack` (test runner).

---

## File Map

**Modified:**
- `crates/rouchdb-core/src/lib.rs` — add `MaybeSend`/`MaybeSync` traits
- `crates/rouchdb-core/src/adapter.rs` — `cfg_attr` on trait + remove hard `Send+Sync` bound
- `crates/rouchdb-adapter-memory/src/lib.rs` — `cfg_attr` on `impl Adapter`
- `crates/rouchdb-adapter-http/Cargo.toml` — conditional reqwest features
- `crates/rouchdb-adapter-http/src/lib.rs` — `cfg_attr` on `impl Adapter`
- `crates/rouchdb-replication/Cargo.toml` — add WASM-only deps, gate tokio-util
- `crates/rouchdb-replication/src/protocol.rs` — gate `replicate_live` / `ReplicationHandle`
- `crates/rouchdb-replication/src/lib.rs` — gate re-exports of `replicate_live`/`ReplicationHandle`
- `crates/rouchdb/src/lib.rs` — `cfg_attr` on `Plugin` trait; gate `RedbAdapter` re-export + `Database::open()`
- `Cargo.toml` (workspace root) — add new member
- `.github/workflows/ci.yml` — add `test-wasm` job

**Created:**
- `crates/rouchdb-adapter-indexeddb/Cargo.toml`
- `crates/rouchdb-adapter-indexeddb/src/lib.rs` — `IndexedDbAdapter` struct + all `Adapter` impl methods
- `crates/rouchdb-adapter-indexeddb/src/schema.rs` — `open_db()`, schema upgrade, `StoredDoc`/`StoredRev`/`StoredChange`/`StoredLocalDoc` types
- `crates/rouchdb-adapter-indexeddb/src/util.rs` — `generate_rev_hash`, `parse_rev`, `compute_attachment_digest`, `idb_err`

---

## Task 1: `rouchdb-core` — Add `MaybeSend`/`MaybeSync` and update `Adapter` trait

**Files:**
- Modify: `crates/rouchdb-core/src/lib.rs`
- Modify: `crates/rouchdb-core/src/adapter.rs`

- [ ] **Step 1: Add `MaybeSend` and `MaybeSync` to `rouchdb-core/src/lib.rs`**

Replace the entire file content with:

```rust
pub mod adapter;
pub mod collation;
pub mod document;
pub mod error;
pub mod merge;
pub mod rev_tree;

// ---------------------------------------------------------------------------
// WASM-compatible Send/Sync bounds
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync> MaybeSync for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSync for T {}
```

- [ ] **Step 2: Update `Adapter` trait declaration in `crates/rouchdb-core/src/adapter.rs`**

Change the top of the file from:
```rust
use async_trait::async_trait;
// ...
#[async_trait]
pub trait Adapter: Send + Sync {
```
to:
```rust
use async_trait::async_trait;
// ...
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Adapter: crate::MaybeSend + crate::MaybeSync {
```

All method signatures remain unchanged.

- [ ] **Step 3: Run existing tests to verify nothing broke**

```bash
cargo test -p rouchdb-core
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rouchdb-core/src/lib.rs crates/rouchdb-core/src/adapter.rs
git commit -m "feat(core): add MaybeSend/MaybeSync, make Adapter trait WASM-compatible"
```

---

## Task 2: Update `MemoryAdapter` impl to match new trait declaration

**Files:**
- Modify: `crates/rouchdb-adapter-memory/src/lib.rs`

- [ ] **Step 1: Update the `async_trait` import and `impl Adapter` annotation**

Change line 4:
```rust
use async_trait::async_trait;
```
to:
```rust
use async_trait::async_trait;
```
(no change to import — `async_trait` macro handles both cases)

Change line 119:
```rust
#[async_trait]
impl Adapter for MemoryAdapter {
```
to:
```rust
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Adapter for MemoryAdapter {
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p rouchdb-adapter-memory
```

Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rouchdb-adapter-memory/src/lib.rs
git commit -m "feat(adapter-memory): update async_trait for WASM compatibility"
```

---

## Task 3: `rouchdb-adapter-http` — Conditional `reqwest` features and `async_trait` update

**Files:**
- Modify: `crates/rouchdb-adapter-http/Cargo.toml`
- Modify: `crates/rouchdb-adapter-http/src/lib.rs`

- [ ] **Step 1: Update `Cargo.toml` for target-conditional reqwest features**

Replace:
```toml
reqwest = { version = "0.12", features = ["json", "cookies"] }
```
with:
```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
reqwest = { version = "0.12", features = ["json", "cookies"] }

[target.'cfg(target_arch = "wasm32")'.dependencies]
reqwest = { version = "0.12", features = ["json"] }
```

And remove the existing `reqwest` line from `[dependencies]`.

- [ ] **Step 2: Update `impl Adapter for HttpAdapter` annotation in `src/lib.rs`**

Change line 229:
```rust
#[async_trait]
impl Adapter for HttpAdapter {
```
to:
```rust
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Adapter for HttpAdapter {
```

- [ ] **Step 3: Verify native build**

```bash
cargo build -p rouchdb-adapter-http
```

Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rouchdb-adapter-http/Cargo.toml crates/rouchdb-adapter-http/src/lib.rs
git commit -m "feat(adapter-http): conditional reqwest features and WASM-compatible async_trait"
```

---

## Task 4: `rouchdb-replication` — Gate `replicate_live` and `ReplicationHandle` for WASM

`replicate_live` uses `tokio::spawn`, `tokio::time::sleep`, and `tokio_util::sync::CancellationToken` — none of which are available on WASM. Gate the entire live-replication surface behind `#[cfg(not(target_arch = "wasm32"))]`. One-shot `replicate()` needs no changes.

**Files:**
- Modify: `crates/rouchdb-replication/Cargo.toml`
- Modify: `crates/rouchdb-replication/src/protocol.rs`
- Modify: `crates/rouchdb-replication/src/lib.rs`

- [ ] **Step 1: Gate `tokio-util` in `Cargo.toml`**

In `crates/rouchdb-replication/Cargo.toml`, move `tokio-util` from `[dependencies]` to a target-conditional block:

```toml
[dependencies]
rouchdb-core = { path = "../rouchdb-core", version = "0.3.2" }
rouchdb-query = { path = "../rouchdb-query", version = "0.3.2" }
md-5 = "0.10"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["sync", "macros", "rt"] }
uuid = { version = "1", features = ["v4"] }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["time"] }
tokio-util = "0.7"
```

Note: keep `tokio` in `[dependencies]` with just the WASM-safe features; the `time` feature and `tokio-util` go under the native-only target.

- [ ] **Step 2: Gate imports and `replicate_live`/`ReplicationHandle` in `protocol.rs`**

At the top of `crates/rouchdb-replication/src/protocol.rs`, gate the `tokio_util` import:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rouchdb_core::adapter::Adapter;
use rouchdb_core::document::*;
use rouchdb_core::error::Result;
use tokio::sync::mpsc;

#[cfg(not(target_arch = "wasm32"))]
use tokio_util::sync::CancellationToken;
```

Then gate the entire `replicate_live` function and `ReplicationHandle` struct with `#[cfg(not(target_arch = "wasm32"))]`:

```rust
/// Handle for a live replication task. Dropping this cancels the replication.
#[cfg(not(target_arch = "wasm32"))]
pub struct ReplicationHandle {
    cancel: CancellationToken,
}

#[cfg(not(target_arch = "wasm32"))]
impl ReplicationHandle {
    /// Cancel the live replication.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for ReplicationHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Run live (continuous) replication. Not available on WASM.
#[cfg(not(target_arch = "wasm32"))]
pub fn replicate_live(
    source: Arc<dyn Adapter>,
    target: Arc<dyn Adapter>,
    opts: ReplicationOptions,
) -> (mpsc::Receiver<ReplicationEvent>, ReplicationHandle) {
    // ... (existing body unchanged)
}
```

- [ ] **Step 3: Gate re-exports in `src/lib.rs`**

```rust
pub use checkpoint::Checkpointer;
pub use protocol::{
    ReplicationEvent, ReplicationFilter, ReplicationOptions, ReplicationResult,
    replicate, replicate_with_events,
};

#[cfg(not(target_arch = "wasm32"))]
pub use protocol::{ReplicationHandle, replicate_live};
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p rouchdb-replication
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-replication/Cargo.toml crates/rouchdb-replication/src/protocol.rs crates/rouchdb-replication/src/lib.rs
git commit -m "feat(replication): gate replicate_live and tokio-util for WASM compatibility"
```

---

## Task 5: `rouchdb` umbrella — `Plugin` trait + gate `RedbAdapter` and `Database::open()`

**Files:**
- Modify: `crates/rouchdb/src/lib.rs`

- [ ] **Step 1: Update `Plugin` trait declaration**

Change:
```rust
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
```
to:
```rust
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Plugin: rouchdb_core::MaybeSend + rouchdb_core::MaybeSync {
```

- [ ] **Step 2: Gate `RedbAdapter` re-export and `Database::open()`**

Find and wrap:
```rust
pub use rouchdb_adapter_redb::RedbAdapter;
```
with:
```rust
#[cfg(not(target_arch = "wasm32"))]
pub use rouchdb_adapter_redb::RedbAdapter;
```

Find `Database::open()` and wrap with:
```rust
#[cfg(not(target_arch = "wasm32"))]
pub fn open(path: impl AsRef<Path>, name: &str) -> Result<Self> {
    let adapter = RedbAdapter::open(path, name)?;
    Ok(Self {
        adapter: Arc::new(adapter),
        indexes: Arc::new(RwLock::new(HashMap::new())),
        plugins: Vec::new(),
    })
}
```

Also gate the `std::path::Path` import:
```rust
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
```

And gate the `replicate_live`/`ReplicationHandle` re-exports from `rouchdb-replication`:
```rust
pub use rouchdb_replication::{
    ReplicationEvent, ReplicationFilter, ReplicationHandle, ReplicationOptions, ReplicationResult,
    replicate, replicate_live, replicate_with_events,
};
```
becomes:
```rust
pub use rouchdb_replication::{
    ReplicationEvent, ReplicationFilter, ReplicationOptions, ReplicationResult,
    replicate, replicate_with_events,
};
#[cfg(not(target_arch = "wasm32"))]
pub use rouchdb_replication::{ReplicationHandle, replicate_live};
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p rouchdb
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rouchdb/src/lib.rs
git commit -m "feat(rouchdb): WASM-compatible Plugin trait, gate RedbAdapter and replicate_live"
```

---

## Task 6: Workspace registration and new crate scaffold

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/rouchdb-adapter-indexeddb/Cargo.toml`
- Create: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Add new member to workspace `Cargo.toml`**

In the root `Cargo.toml`, add `"crates/rouchdb-adapter-indexeddb"` to the `members` array.

- [ ] **Step 2: Create `crates/rouchdb-adapter-indexeddb/Cargo.toml`**

```toml
[package]
name = "rouchdb-adapter-indexeddb"
description = "IndexedDB storage adapter for RouchDB (WASM only)"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
keywords.workspace = true
categories.workspace = true
readme.workspace = true

[dependencies]
rouchdb-core = { path = "../rouchdb-core", version = "0.3.2" }
async-trait = "0.1"
base64 = "0.22"
md-5 = "0.10"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
idb = "0.6"
js-sys = "0.3"
serde-wasm-bindgen = "0.6"
uuid = { version = "1", features = ["v4", "js"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
wasm-bindgen-test = "0.3"
```

- [ ] **Step 3: Create `crates/rouchdb-adapter-indexeddb/src/lib.rs`**

```rust
#![cfg(target_arch = "wasm32")]

mod schema;
mod util;

use std::cell::RefCell;
use std::collections::HashMap;

use async_trait::async_trait;
use rouchdb_core::adapter::Adapter;
use rouchdb_core::document::*;
use rouchdb_core::error::{Result, RouchError};

pub use schema::IndexedDbAdapter;
```

- [ ] **Step 4: Verify native build still passes**

```bash
cargo build --workspace
```

Expected: compiles without errors (the indexeddb crate compiles as an empty lib on native).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/rouchdb-adapter-indexeddb/
git commit -m "feat(adapter-indexeddb): add crate scaffold and workspace registration"
```

---

## Task 7: IndexedDB schema — `schema.rs` with storage types and `open_db()`

**Files:**
- Create: `crates/rouchdb-adapter-indexeddb/src/schema.rs`

- [ ] **Step 1: Write the failing test (open and close a database)**

Add to the bottom of `crates/rouchdb-adapter-indexeddb/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use wasm_bindgen_test::*;
    wasm_bindgen_test_configure!(run_in_browser);

    use super::schema::IndexedDbAdapter;

    #[wasm_bindgen_test]
    async fn open_database() {
        let adapter = IndexedDbAdapter::open("test-open").await;
        assert!(adapter.is_ok(), "failed to open: {:?}", adapter.err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: compile error — `schema` module not found.

- [ ] **Step 3: Implement `schema.rs`**

Create `crates/rouchdb-adapter-indexeddb/src/schema.rs`:

```rust
use idb::{Database, Factory, KeyPath, ObjectStoreParams};
use serde::{Deserialize, Serialize};

use rouchdb_core::rev_tree::RevTree;

use crate::util::idb_err;
use rouchdb_core::error::Result;

// ---------------------------------------------------------------------------
// Internal storage types
// ---------------------------------------------------------------------------

/// One record per document in the `docs` store. Key: `id`.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredDoc {
    pub id: String,
    /// JSON-serialized RevTree
    pub rev_tree: String,
    pub seq: u64,
}

/// One record per revision body in the `revs` store.
/// Key (out-of-line): `"<doc_id>\x00<rev_string>"`.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredRev {
    pub doc_id: String,
    pub rev: String,
    /// JSON-serialized document body (serde_json::Value)
    pub data: String,
    pub deleted: bool,
}

/// One record per document write in the `changes` store. Key: `seq`.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredChange {
    pub seq: u64,
    pub doc_id: String,
    pub deleted: bool,
}

/// One record per local doc in the `local_docs` store. Key: `id`.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredLocalDoc {
    pub id: String,
    /// JSON-serialized serde_json::Value
    pub data: String,
}

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

pub struct IndexedDbAdapter {
    pub(crate) db: Database,
    pub(crate) name: String,
    /// Cached update_seq — loaded from `meta` store on open, updated on every write.
    /// RefCell is safe because WASM is single-threaded.
    pub(crate) update_seq: RefCell<u64>,
}

use std::cell::RefCell;

impl IndexedDbAdapter {
    /// Open (or create) a named IndexedDB database and return an adapter.
    pub async fn open(name: &str) -> Result<Self> {
        let factory = Factory::new().map_err(idb_err)?;
        let mut open_request = factory.open(name, Some(1)).map_err(idb_err)?;

        open_request.on_upgrade_needed(|event| {
            let db = event.database().map_err(idb_err)?;

            // docs: keyed by "id"
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("id")));
            db.create_object_store("docs", p).map_err(idb_err)?;

            // revs: out-of-line key ("<doc_id>\x00<rev>")
            db.create_object_store("revs", ObjectStoreParams::new()).map_err(idb_err)?;

            // changes: keyed by "seq"
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("seq")));
            db.create_object_store("changes", p).map_err(idb_err)?;

            // local_docs: keyed by "id"
            let mut p = ObjectStoreParams::new();
            p.key_path(Some(KeyPath::new_single("id")));
            db.create_object_store("local_docs", p).map_err(idb_err)?;

            // attachments: out-of-line key (digest string)
            db.create_object_store("attachments", ObjectStoreParams::new()).map_err(idb_err)?;

            // meta: out-of-line key (string key name)
            db.create_object_store("meta", ObjectStoreParams::new()).map_err(idb_err)?;

            Ok(())
        });

        let db: Database = open_request.await.map_err(idb_err)?;

        // Load persisted update_seq from meta store
        let update_seq = load_update_seq(&db).await?;

        Ok(Self {
            db,
            name: name.to_string(),
            update_seq: RefCell::new(update_seq),
        })
    }
}

async fn load_update_seq(db: &Database) -> Result<u64> {
    use idb::TransactionMode;
    use wasm_bindgen::JsValue;

    let txn = db.transaction(&["meta"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let store = txn.object_store("meta").map_err(idb_err)?;
    let result = store.get(JsValue::from_str("update_seq")).map_err(idb_err)?.await.map_err(idb_err)?;

    Ok(match result {
        Some(val) => {
            let s: String = serde_wasm_bindgen::from_value(val)
                .map_err(|e| RouchError::DatabaseError(e.to_string()))?;
            s.parse().unwrap_or(0)
        }
        None => 0,
    })
}

/// Persist update_seq to the meta store inside an already-open transaction.
pub(crate) fn rev_key(doc_id: &str, rev: &str) -> String {
    format!("{}\x00{}", doc_id, rev)
}
```

- [ ] **Step 4: Wire `schema` into `lib.rs`**

Ensure `lib.rs` declares `pub(crate) mod schema;` and the `use` statements compile.

- [ ] **Step 5: Run test to verify it passes**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: `open_database` passes.

- [ ] **Step 6: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/schema.rs crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): IndexedDB schema setup and open_db"
```

---

## Task 8: Utility helpers — `util.rs`

**Files:**
- Create: `crates/rouchdb-adapter-indexeddb/src/util.rs`

- [ ] **Step 1: Write `util.rs`**

```rust
use md5::{Digest, Md5};
use rouchdb_core::error::{Result, RouchError};

/// Map an `idb::Error` to `RouchError`.
pub(crate) fn idb_err(e: idb::Error) -> RouchError {
    RouchError::DatabaseError(e.to_string())
}

/// Map a `serde_wasm_bindgen` error to `RouchError`.
pub(crate) fn serde_err(e: serde_wasm_bindgen::Error) -> RouchError {
    RouchError::DatabaseError(e.to_string())
}

/// Map a `wasm_bindgen::JsValue` error to `RouchError`.
pub(crate) fn js_err(e: wasm_bindgen::JsValue) -> RouchError {
    RouchError::DatabaseError(format!("{:?}", e))
}

/// Generate a CouchDB-style MD5 revision hash from document content.
pub(crate) fn generate_rev_hash(
    doc_data: &serde_json::Value,
    deleted: bool,
    prev_rev: Option<&str>,
) -> String {
    let mut hasher = Md5::new();
    if let Some(prev) = prev_rev {
        hasher.update(prev.as_bytes());
    }
    hasher.update(if deleted { b"1" } else { b"0" });
    let serialized = serde_json::to_string(doc_data).unwrap_or_default();
    hasher.update(serialized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Parse a `"{pos}-{hash}"` revision string.
pub(crate) fn parse_rev(rev_str: &str) -> Result<(u64, String)> {
    let (pos_str, hash) = rev_str
        .split_once('-')
        .ok_or_else(|| RouchError::InvalidRev(rev_str.to_string()))?;
    let pos: u64 = pos_str
        .parse()
        .map_err(|_| RouchError::InvalidRev(rev_str.to_string()))?;
    Ok((pos, hash.to_string()))
}

/// Compute an MD5 content-addressed attachment digest.
pub(crate) fn compute_attachment_digest(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    let hash = hasher.finalize();
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(hash);
    format!("md5-{}", b64)
}
```

- [ ] **Step 2: Verify compile**

```bash
wasm-pack build crates/rouchdb-adapter-indexeddb
```

Expected: compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/util.rs
git commit -m "feat(adapter-indexeddb): add utility helpers (rev hash, parse_rev, idb_err)"
```

---

## Task 9: `info()`, `get_local()`, `put_local()`, `remove_local()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing tests**

Add to the test module in `lib.rs`:

```rust
#[wasm_bindgen_test]
async fn info_empty_db() {
    let db = IndexedDbAdapter::open("test-info").await.unwrap();
    let info = db.info().await.unwrap();
    assert_eq!(info.db_name, "test-info");
    assert_eq!(info.doc_count, 0);
    assert_eq!(info.update_seq, Seq::Num(0));
}

#[wasm_bindgen_test]
async fn local_docs_crud() {
    let db = IndexedDbAdapter::open("test-local").await.unwrap();
    let val = serde_json::json!({"checkpoint": 42});
    db.put_local("repl-1", val.clone()).await.unwrap();
    let fetched = db.get_local("repl-1").await.unwrap();
    assert_eq!(fetched["checkpoint"], 42);
    db.remove_local("repl-1").await.unwrap();
    assert!(db.get_local("repl-1").await.is_err());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: compile error — `IndexedDbAdapter` does not implement `Adapter`.

- [ ] **Step 3: Implement `Adapter` for `IndexedDbAdapter` with these four methods**

Add an `impl Adapter for IndexedDbAdapter` block in `lib.rs`. Start with just the four methods below; leave others as `todo!()` stubs temporarily:

```rust
#[async_trait(?Send)]
impl Adapter for IndexedDbAdapter {
    async fn info(&self) -> Result<DbInfo> {
        use idb::TransactionMode;
        use wasm_bindgen::JsValue;

        let txn = self.db.transaction(&["docs"], TransactionMode::ReadOnly)
            .map_err(idb_err)?;
        let docs_store = txn.object_store("docs").map_err(idb_err)?;
        // Count non-deleted docs
        let all_vals = docs_store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?;
        let doc_count = all_vals.iter().filter(|v| {
            serde_wasm_bindgen::from_value::<schema::StoredDoc>((*v).clone())
                .map(|d| {
                    let tree: rouchdb_core::rev_tree::RevTree =
                        serde_json::from_str(&d.rev_tree).unwrap_or_default();
                    !rouchdb_core::merge::is_deleted(&tree)
                })
                .unwrap_or(false)
        }).count() as u64;

        Ok(DbInfo {
            db_name: self.name.clone(),
            doc_count,
            update_seq: Seq::Num(*self.update_seq.borrow()),
        })
    }

    async fn get_local(&self, id: &str) -> Result<serde_json::Value> {
        use idb::TransactionMode;
        use wasm_bindgen::JsValue;

        let txn = self.db.transaction(&["local_docs"], TransactionMode::ReadOnly)
            .map_err(idb_err)?;
        let store = txn.object_store("local_docs").map_err(idb_err)?;
        let result = store.get(JsValue::from_str(id)).map_err(idb_err)?.await.map_err(idb_err)?;
        match result {
            None => Err(RouchError::NotFound(format!("_local/{}", id))),
            Some(val) => {
                let stored: schema::StoredLocalDoc =
                    serde_wasm_bindgen::from_value(val).map_err(serde_err)?;
                serde_json::from_str(&stored.data)
                    .map_err(|e| RouchError::DatabaseError(e.to_string()))
            }
        }
    }

    async fn put_local(&self, id: &str, doc: serde_json::Value) -> Result<()> {
        use idb::TransactionMode;
        use wasm_bindgen::JsValue;

        let stored = schema::StoredLocalDoc {
            id: id.to_string(),
            data: serde_json::to_string(&doc)
                .map_err(|e| RouchError::DatabaseError(e.to_string()))?,
        };
        let js_val = serde_wasm_bindgen::to_value(&stored).map_err(serde_err)?;
        let txn = self.db.transaction(&["local_docs"], TransactionMode::ReadWrite)
            .map_err(idb_err)?;
        let store = txn.object_store("local_docs").map_err(idb_err)?;
        store.put(&js_val, None).map_err(idb_err)?.await.map_err(idb_err)?;
        txn.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        Ok(())
    }

    async fn remove_local(&self, id: &str) -> Result<()> {
        use idb::TransactionMode;
        use wasm_bindgen::JsValue;

        // Verify it exists first
        self.get_local(id).await?;
        let txn = self.db.transaction(&["local_docs"], TransactionMode::ReadWrite)
            .map_err(idb_err)?;
        let store = txn.object_store("local_docs").map_err(idb_err)?;
        store.delete(JsValue::from_str(id)).map_err(idb_err)?.await.map_err(idb_err)?;
        txn.commit().map_err(idb_err)?.await.map_err(idb_err)?;
        Ok(())
    }

    // Stub remaining methods to compile:
    async fn get(&self, _id: &str, _opts: GetOptions) -> Result<Document> { todo!() }
    async fn bulk_docs(&self, _docs: Vec<Document>, _opts: BulkDocsOptions) -> Result<Vec<DocResult>> { todo!() }
    async fn all_docs(&self, _opts: AllDocsOptions) -> Result<AllDocsResponse> { todo!() }
    async fn changes(&self, _opts: ChangesOptions) -> Result<ChangesResponse> { todo!() }
    async fn revs_diff(&self, _revs: std::collections::HashMap<String, Vec<String>>) -> Result<RevsDiffResponse> { todo!() }
    async fn bulk_get(&self, _docs: Vec<BulkGetItem>) -> Result<BulkGetResponse> { todo!() }
    async fn put_attachment(&self, _doc_id: &str, _att_id: &str, _rev: &str, _data: Vec<u8>, _content_type: &str) -> Result<DocResult> { todo!() }
    async fn get_attachment(&self, _doc_id: &str, _att_id: &str, _opts: GetAttachmentOptions) -> Result<Vec<u8>> { todo!() }
    async fn remove_attachment(&self, _doc_id: &str, _att_id: &str, _rev: &str) -> Result<DocResult> { todo!() }
    async fn compact(&self) -> Result<()> { todo!() }
    async fn destroy(&self) -> Result<()> { todo!() }
}
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: `info_empty_db` and `local_docs_crud` pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement info, get_local, put_local, remove_local"
```

---

## Task 10: `bulk_docs()` — `new_edits=true` (normal write path)

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[wasm_bindgen_test]
async fn create_and_get_document() {
    let db = IndexedDbAdapter::open("test-bulk-write").await.unwrap();
    let doc = Document {
        id: "doc1".into(), rev: None, deleted: false,
        data: serde_json::json!({"name": "Alice"}),
        attachments: std::collections::HashMap::new(),
    };
    let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    assert!(results[0].ok, "expected ok: {:?}", results[0]);
    assert_eq!(results[0].id, "doc1");
    assert!(results[0].rev.is_some());
}

#[wasm_bindgen_test]
async fn conflict_on_wrong_rev() {
    let db = IndexedDbAdapter::open("test-conflict").await.unwrap();
    let doc = Document {
        id: "doc1".into(), rev: None, deleted: false,
        data: serde_json::json!({"v": 1}),
        attachments: std::collections::HashMap::new(),
    };
    db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    let bad = Document {
        id: "doc1".into(),
        rev: Some(Revision::new(1, "wronghash".into())),
        deleted: false,
        data: serde_json::json!({"v": 2}),
        attachments: std::collections::HashMap::new(),
    };
    let results = db.bulk_docs(vec![bad], BulkDocsOptions::new()).await.unwrap();
    assert!(!results[0].ok);
    assert_eq!(results[0].error.as_deref(), Some("conflict"));
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: tests panic at `todo!()` in `bulk_docs`.

- [ ] **Step 3: Implement `bulk_docs` in `lib.rs`**

Replace the stub with:

```rust
async fn bulk_docs(&self, docs: Vec<Document>, opts: BulkDocsOptions) -> Result<Vec<DocResult>> {
    let mut results = Vec::with_capacity(docs.len());
    for doc in docs {
        let result = if opts.new_edits {
            self.process_doc_new_edits(doc).await
        } else {
            self.process_doc_replication(doc).await
        };
        results.push(result);
    }
    Ok(results)
}
```

Add a private `process_doc_new_edits` method on `IndexedDbAdapter`:

```rust
impl IndexedDbAdapter {
    async fn process_doc_new_edits(&self, doc: Document) -> DocResult {
        use idb::TransactionMode;
        use rouchdb_core::merge::{collect_conflicts, is_deleted, merge_tree, winning_rev};
        use rouchdb_core::rev_tree::{NodeOpts, RevStatus, build_path_from_revs};
        use wasm_bindgen::JsValue;

        const REV_LIMIT: u64 = 1000;

        let doc_id = if doc.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            doc.id.clone()
        };

        // Load existing doc record if any
        let txn = match self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err) {
            Ok(t) => t, Err(e) => return DocResult { ok: false, id: doc_id, rev: None, error: Some("db_error".into()), reason: Some(e.to_string()) },
        };
        let docs_store = txn.object_store("docs").map_err(idb_err).unwrap();
        let existing_js = docs_store.get(JsValue::from_str(&doc_id)).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();
        let existing: Option<schema::StoredDoc> = existing_js.and_then(|v| serde_wasm_bindgen::from_value(v).ok());

        let existing_tree = existing.as_ref()
            .and_then(|d| serde_json::from_str::<rouchdb_core::rev_tree::RevTree>(&d.rev_tree).ok())
            .unwrap_or_default();

        let winner = winning_rev(&existing_tree);

        // Conflict check
        match (&doc.rev, &winner) {
            (Some(provided), Some(current)) => {
                if provided.to_string() != current.to_string() {
                    return DocResult { ok: false, id: doc_id, rev: None, error: Some("conflict".into()), reason: Some("Document update conflict".into()) };
                }
            }
            (None, Some(_)) => {
                if !is_deleted(&existing_tree) {
                    return DocResult { ok: false, id: doc_id, rev: None, error: Some("conflict".into()), reason: Some("Document update conflict".into()) };
                }
            }
            (Some(_), None) => {
                return DocResult { ok: false, id: doc_id, rev: None, error: Some("not_found".into()), reason: Some("missing".into()) };
            }
            _ => {}
        }

        // Generate new revision
        let new_pos = doc.rev.as_ref().map(|r| r.pos + 1).unwrap_or(1);
        let prev_rev_str = doc.rev.as_ref().map(|r| r.to_string());
        let new_hash = util::generate_rev_hash(&doc.data, doc.deleted, prev_rev_str.as_deref());
        let new_rev_str = format!("{}-{}", new_pos, new_hash);

        let mut rev_hashes = vec![new_hash.clone()];
        if let Some(ref prev) = doc.rev { rev_hashes.push(prev.hash.clone()); }

        let new_path = build_path_from_revs(
            new_pos, &rev_hashes,
            NodeOpts { deleted: doc.deleted },
            RevStatus::Available,
        );
        let (merged_tree, _) = merge_tree(&existing_tree, &new_path, REV_LIMIT);
        let merged_tree_json = serde_json::to_string(&merged_tree).unwrap_or_default();

        // Bump update_seq
        let seq = {
            let mut s = self.update_seq.borrow_mut();
            *s += 1;
            *s
        };

        // Write: docs + revs + changes + meta in one transaction
        let txn = match self.db.transaction(
            &["docs", "revs", "changes", "meta"], TransactionMode::ReadWrite,
        ).map_err(idb_err) {
            Ok(t) => t, Err(e) => return DocResult { ok: false, id: doc_id, rev: None, error: Some("db_error".into()), reason: Some(e.to_string()) },
        };

        let docs_store = txn.object_store("docs").map_err(idb_err).unwrap();
        let revs_store = txn.object_store("revs").map_err(idb_err).unwrap();
        let changes_store = txn.object_store("changes").map_err(idb_err).unwrap();
        let meta_store = txn.object_store("meta").map_err(idb_err).unwrap();

        // Remove old changes entry for this doc
        if let Some(ref ex) = existing {
            let old_change_key = JsValue::from_f64(ex.seq as f64);
            let _ = changes_store.delete(old_change_key).map_err(idb_err).unwrap().await;
        }

        // Write doc record
        let stored_doc = schema::StoredDoc { id: doc_id.clone(), rev_tree: merged_tree_json, seq };
        let doc_js = serde_wasm_bindgen::to_value(&stored_doc).unwrap();
        docs_store.put(&doc_js, None).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

        // Write rev body
        let stored_rev = schema::StoredRev {
            doc_id: doc_id.clone(),
            rev: new_rev_str.clone(),
            data: serde_json::to_string(&doc.data).unwrap_or_default(),
            deleted: doc.deleted,
        };
        let rev_js = serde_wasm_bindgen::to_value(&stored_rev).unwrap();
        let rev_key = JsValue::from_str(&schema::rev_key(&doc_id, &new_rev_str));
        revs_store.put(&rev_js, Some(&rev_key)).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

        // Write change entry
        let stored_change = schema::StoredChange { seq, doc_id: doc_id.clone(), deleted: doc.deleted };
        let change_js = serde_wasm_bindgen::to_value(&stored_change).unwrap();
        changes_store.put(&change_js, Some(&JsValue::from_f64(seq as f64))).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

        // Persist update_seq to meta
        let seq_js = serde_wasm_bindgen::to_value(&seq.to_string()).unwrap();
        meta_store.put(&seq_js, Some(&JsValue::from_str("update_seq"))).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

        txn.commit().map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

        DocResult { ok: true, id: doc_id, rev: Some(new_rev_str), error: None, reason: None }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: `create_and_get_document` and `conflict_on_wrong_rev` pass (note: `create_and_get_document` only tests `bulk_docs`, not `get` yet).

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): bulk_docs new_edits=true"
```

---

## Task 11: `bulk_docs()` — `new_edits=false` (replication mode)

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
#[wasm_bindgen_test]
async fn replication_mode_bulk_docs() {
    let db = IndexedDbAdapter::open("test-repl-mode").await.unwrap();
    let doc = Document {
        id: "doc1".into(),
        rev: Some(Revision::new(1, "abc123".into())),
        deleted: false,
        data: serde_json::json!({"name": "replicated"}),
        attachments: std::collections::HashMap::new(),
    };
    let results = db.bulk_docs(vec![doc], BulkDocsOptions::replication()).await.unwrap();
    assert!(results[0].ok);
    assert_eq!(results[0].rev.as_deref(), Some("1-abc123"));
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: panics at `todo!()` in `process_doc_replication`.

- [ ] **Step 3: Implement `process_doc_replication`**

Add to the `impl IndexedDbAdapter` block:

```rust
async fn process_doc_replication(&self, mut doc: Document) -> DocResult {
    use idb::TransactionMode;
    use rouchdb_core::merge::{is_deleted, merge_tree};
    use rouchdb_core::rev_tree::{NodeOpts, RevPath, RevStatus, RevNode, build_path_from_revs};
    use wasm_bindgen::JsValue;

    const REV_LIMIT: u64 = 1000;

    let doc_id = doc.id.clone();
    let rev = match &doc.rev {
        Some(r) => r.clone(),
        None => return DocResult { ok: false, id: doc_id, rev: None, error: Some("bad_request".into()), reason: Some("missing _rev".into()) },
    };
    let rev_str = rev.to_string();

    // Build rev path from _revisions ancestry if present
    let new_path = if let Some(revisions) = doc.data.get("_revisions") {
        let start = revisions["start"].as_u64().unwrap_or(rev.pos);
        let ids: Vec<String> = revisions["ids"].as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_else(|| vec![rev.hash.clone()]);
        build_path_from_revs(start, &ids, NodeOpts { deleted: doc.deleted }, RevStatus::Available)
    } else {
        RevPath {
            pos: rev.pos,
            tree: RevNode { hash: rev.hash.clone(), status: RevStatus::Available, opts: NodeOpts { deleted: doc.deleted }, children: vec![] },
        }
    };

    // Strip _revisions from stored data
    if let serde_json::Value::Object(ref mut map) = doc.data { map.remove("_revisions"); }

    // Load existing tree
    let txn = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err).unwrap();
    let docs_store = txn.object_store("docs").map_err(idb_err).unwrap();
    let existing_js = docs_store.get(JsValue::from_str(&doc_id)).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();
    let existing: Option<schema::StoredDoc> = existing_js.and_then(|v| serde_wasm_bindgen::from_value(v).ok());
    let existing_tree = existing.as_ref()
        .and_then(|d| serde_json::from_str::<rouchdb_core::rev_tree::RevTree>(&d.rev_tree).ok())
        .unwrap_or_default();

    let (merged_tree, _) = merge_tree(&existing_tree, &new_path, REV_LIMIT);
    let merged_tree_json = serde_json::to_string(&merged_tree).unwrap_or_default();
    let is_doc_deleted = is_deleted(&merged_tree);

    let seq = { let mut s = self.update_seq.borrow_mut(); *s += 1; *s };

    let txn = self.db.transaction(&["docs", "revs", "changes", "meta"], TransactionMode::ReadWrite).map_err(idb_err).unwrap();
    let docs_store = txn.object_store("docs").map_err(idb_err).unwrap();
    let revs_store = txn.object_store("revs").map_err(idb_err).unwrap();
    let changes_store = txn.object_store("changes").map_err(idb_err).unwrap();
    let meta_store = txn.object_store("meta").map_err(idb_err).unwrap();

    if let Some(ref ex) = existing {
        let _ = changes_store.delete(JsValue::from_f64(ex.seq as f64)).map_err(idb_err).unwrap().await;
    }

    let stored_doc = schema::StoredDoc { id: doc_id.clone(), rev_tree: merged_tree_json, seq };
    docs_store.put(&serde_wasm_bindgen::to_value(&stored_doc).unwrap(), None).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

    let stored_rev = schema::StoredRev { doc_id: doc_id.clone(), rev: rev_str.clone(), data: serde_json::to_string(&doc.data).unwrap_or_default(), deleted: doc.deleted };
    let rev_key = JsValue::from_str(&schema::rev_key(&doc_id, &rev_str));
    revs_store.put(&serde_wasm_bindgen::to_value(&stored_rev).unwrap(), Some(&rev_key)).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

    let stored_change = schema::StoredChange { seq, doc_id: doc_id.clone(), deleted: is_doc_deleted };
    changes_store.put(&serde_wasm_bindgen::to_value(&stored_change).unwrap(), Some(&JsValue::from_f64(seq as f64))).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

    meta_store.put(&serde_wasm_bindgen::to_value(&seq.to_string()).unwrap(), Some(&JsValue::from_str("update_seq"))).map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

    txn.commit().map_err(idb_err).unwrap().await.map_err(idb_err).unwrap();

    DocResult { ok: true, id: doc_id, rev: Some(rev_str), error: None, reason: None }
}
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: `replication_mode_bulk_docs` passes, all previous tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): bulk_docs new_edits=false (replication mode)"
```

---

## Task 12: `get()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[wasm_bindgen_test]
async fn get_document() {
    let db = IndexedDbAdapter::open("test-get").await.unwrap();
    let doc = Document {
        id: "alice".into(), rev: None, deleted: false,
        data: serde_json::json!({"name": "Alice"}),
        attachments: std::collections::HashMap::new(),
    };
    db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    let fetched = db.get("alice", GetOptions::default()).await.unwrap();
    assert_eq!(fetched.id, "alice");
    assert_eq!(fetched.data["name"], "Alice");
}

#[wasm_bindgen_test]
async fn get_missing_document() {
    let db = IndexedDbAdapter::open("test-get-missing").await.unwrap();
    assert!(db.get("nope", GetOptions::default()).await.is_err());
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: panics at `todo!()` in `get`.

- [ ] **Step 3: Implement `get()`**

Replace the stub with:

```rust
async fn get(&self, id: &str, opts: GetOptions) -> Result<Document> {
    use idb::TransactionMode;
    use rouchdb_core::merge::{collect_conflicts, is_deleted, winning_rev};
    use rouchdb_core::rev_tree::{collect_leaves, find_rev_ancestry, traverse_rev_tree};
    use wasm_bindgen::JsValue;

    let txn = self.db.transaction(&["docs", "revs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let docs_store = txn.object_store("docs").map_err(idb_err)?;
    let revs_store = txn.object_store("revs").map_err(idb_err)?;

    let doc_js = docs_store.get(JsValue::from_str(id)).map_err(idb_err)?.await.map_err(idb_err)?
        .ok_or_else(|| RouchError::NotFound(id.to_string()))?;
    let stored: schema::StoredDoc = serde_wasm_bindgen::from_value(doc_js).map_err(serde_err)?;
    let rev_tree: rouchdb_core::rev_tree::RevTree =
        serde_json::from_str(&stored.rev_tree).unwrap_or_default();

    let mut target_rev = if let Some(ref rev_str) = opts.rev {
        rev_str.clone()
    } else {
        winning_rev(&rev_tree)
            .ok_or_else(|| RouchError::NotFound(id.to_string()))?
            .to_string()
    };

    if opts.latest && opts.rev.is_some() {
        let leaves = collect_leaves(&rev_tree);
        if !leaves.iter().any(|l| l.rev_string() == target_rev) {
            if let Some(leaf) = leaves.first() {
                target_rev = leaf.rev_string();
            }
        }
    }

    // Fetch rev body from revs store
    let rev_key = JsValue::from_str(&schema::rev_key(id, &target_rev));
    let rev_js = revs_store.get(rev_key).map_err(idb_err)?.await.map_err(idb_err)?
        .ok_or_else(|| RouchError::NotFound(id.to_string()))?;
    let stored_rev: schema::StoredRev = serde_wasm_bindgen::from_value(rev_js).map_err(serde_err)?;

    let deleted = stored_rev.deleted;
    if deleted && opts.rev.is_none() {
        return Err(RouchError::NotFound(id.to_string()));
    }

    let (pos, hash) = util::parse_rev(&target_rev)?;
    let rev = Revision::new(pos, hash);
    let data: serde_json::Value = serde_json::from_str(&stored_rev.data)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let mut doc = Document { id: id.to_string(), rev: Some(rev), deleted, data, attachments: HashMap::new() };

    if opts.conflicts {
        let conflicts = collect_conflicts(&rev_tree);
        if !conflicts.is_empty() {
            let list: Vec<serde_json::Value> = conflicts.iter().map(|c| serde_json::Value::String(c.to_string())).collect();
            if let serde_json::Value::Object(ref mut map) = doc.data {
                map.insert("_conflicts".into(), serde_json::Value::Array(list));
            }
        }
    }

    if opts.revs_info {
        let mut revs_info = Vec::new();
        traverse_rev_tree(&rev_tree, |node_pos, node, _| {
            revs_info.push(RevInfo {
                rev: format!("{}-{}", node_pos, node.hash),
                status: if node.opts.deleted { "deleted" } else {
                    match node.status { rouchdb_core::rev_tree::RevStatus::Available => "available", _ => "missing" }
                }.into(),
            });
        });
        revs_info.sort_by(|a, b| {
            let a_pos: u64 = a.rev.split('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let b_pos: u64 = b.rev.split('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            b_pos.cmp(&a_pos)
        });
        if let serde_json::Value::Object(ref mut map) = doc.data {
            map.insert("_revs_info".into(), serde_json::to_value(&revs_info).unwrap());
        }
    }

    Ok(doc)
}
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement get()"
```

---

## Task 13: `all_docs()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[wasm_bindgen_test]
async fn all_docs_sorted() {
    let db = IndexedDbAdapter::open("test-all-docs").await.unwrap();
    for name in ["charlie", "alice", "bob"] {
        db.bulk_docs(vec![Document {
            id: name.into(), rev: None, deleted: false,
            data: serde_json::json!({"name": name}),
            attachments: std::collections::HashMap::new(),
        }], BulkDocsOptions::new()).await.unwrap();
    }
    let result = db.all_docs(AllDocsOptions::new()).await.unwrap();
    assert_eq!(result.total_rows, 3);
    assert_eq!(result.rows[0].id, "alice");
    assert_eq!(result.rows[1].id, "bob");
    assert_eq!(result.rows[2].id, "charlie");
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: panics at `todo!()` in `all_docs`.

- [ ] **Step 3: Implement `all_docs()`**

Replace stub with:

```rust
async fn all_docs(&self, opts: AllDocsOptions) -> Result<AllDocsResponse> {
    use idb::TransactionMode;
    use rouchdb_core::merge::{collect_conflicts, is_deleted, winning_rev};

    let txn = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let docs_store = txn.object_store("docs").map_err(idb_err)?;
    let all_vals = docs_store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?;

    let mut stored_docs: Vec<schema::StoredDoc> = all_vals.into_iter()
        .filter_map(|v| serde_wasm_bindgen::from_value(v).ok())
        .collect();
    stored_docs.sort_by(|a, b| a.id.cmp(&b.id));
    if opts.descending { stored_docs.reverse(); }

    let target_keys: Vec<String> = if let Some(ref keys) = opts.keys {
        keys.clone()
    } else if let Some(ref key) = opts.key {
        vec![key.clone()]
    } else {
        stored_docs.iter().map(|d| d.id.clone()).collect()
    };

    let doc_map: std::collections::HashMap<String, schema::StoredDoc> =
        stored_docs.into_iter().map(|d| (d.id.clone(), d)).collect();

    let mut rows = Vec::new();
    for key in &target_keys {
        // Range filters (only when iterating all keys)
        if opts.keys.is_none() && opts.key.is_none() {
            if let Some(ref start) = opts.start_key {
                if (!opts.descending && key.as_str() < start.as_str()) || (opts.descending && key.as_str() > start.as_str()) { continue; }
            }
            if let Some(ref end) = opts.end_key {
                let past_end = if opts.inclusive_end { ((!opts.descending) && key.as_str() > end.as_str()) || (opts.descending && key.as_str() < end.as_str()) }
                    else { ((!opts.descending) && key.as_str() >= end.as_str()) || (opts.descending && key.as_str() <= end.as_str()) };
                if past_end { continue; }
            }
        }

        if let Some(stored) = doc_map.get(key.as_str()) {
            let tree: rouchdb_core::rev_tree::RevTree = serde_json::from_str(&stored.rev_tree).unwrap_or_default();
            let winner = match winning_rev(&tree) { Some(w) => w, None => continue };
            let deleted = is_deleted(&tree);
            if deleted && opts.keys.is_none() { continue; }

            let doc_json = if opts.include_docs && !deleted {
                let txn2 = self.db.transaction(&["revs"], TransactionMode::ReadOnly).map_err(idb_err)?;
                let revs_store = txn2.object_store("revs").map_err(idb_err)?;
                let rev_str = winner.to_string();
                let rev_js = revs_store.get(wasm_bindgen::JsValue::from_str(&schema::rev_key(&stored.id, &rev_str))).map_err(idb_err)?.await.map_err(idb_err)?;
                rev_js.and_then(|v| serde_wasm_bindgen::from_value::<schema::StoredRev>(v).ok()).map(|sr| {
                    let mut obj = match serde_json::from_str::<serde_json::Value>(&sr.data) {
                        Ok(serde_json::Value::Object(m)) => m, _ => serde_json::Map::new(),
                    };
                    obj.insert("_id".into(), serde_json::Value::String(key.clone()));
                    obj.insert("_rev".into(), serde_json::Value::String(rev_str));
                    serde_json::Value::Object(obj)
                })
            } else { None };

            rows.push(AllDocsRow {
                id: key.clone(), key: key.clone(),
                value: AllDocsRowValue { rev: winner.to_string(), deleted: if deleted { Some(true) } else { None } },
                doc: doc_json,
            });
        }
    }

    let total_rows = rows.len() as u64;
    let skip = opts.skip as usize;
    if skip > 0 { rows = rows.into_iter().skip(skip).collect(); }
    if let Some(limit) = opts.limit { rows.truncate(limit as usize); }

    Ok(AllDocsResponse { total_rows, offset: opts.skip, rows, update_seq: None })
}
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement all_docs()"
```

---

## Task 14: `changes()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
#[wasm_bindgen_test]
async fn changes_feed() {
    let db = IndexedDbAdapter::open("test-changes").await.unwrap();
    for i in 0u32..3 {
        db.bulk_docs(vec![Document {
            id: format!("doc{}", i), rev: None, deleted: false,
            data: serde_json::json!({"i": i}),
            attachments: std::collections::HashMap::new(),
        }], BulkDocsOptions::new()).await.unwrap();
    }
    let changes = db.changes(ChangesOptions::default()).await.unwrap();
    assert_eq!(changes.results.len(), 3);
    // Changes since seq 2 should give only the last doc
    let changes2 = db.changes(ChangesOptions { since: Seq::Num(2), ..Default::default() }).await.unwrap();
    assert_eq!(changes2.results.len(), 1);
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

- [ ] **Step 3: Implement `changes()`**

```rust
async fn changes(&self, opts: ChangesOptions) -> Result<ChangesResponse> {
    use idb::TransactionMode;
    use rouchdb_core::merge::winning_rev;

    let txn = self.db.transaction(&["changes", "docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let changes_store = txn.object_store("changes").map_err(idb_err)?;
    let all_vals = changes_store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?;

    let mut all_changes: Vec<schema::StoredChange> = all_vals.into_iter()
        .filter_map(|v| serde_wasm_bindgen::from_value(v).ok())
        .collect();
    all_changes.sort_by_key(|c| c.seq);
    if opts.descending { all_changes.reverse(); }

    let since = opts.since.as_num();
    let docs_store = txn.object_store("docs").map_err(idb_err)?;
    let mut results = Vec::new();

    for entry in all_changes {
        if (!opts.descending && entry.seq <= since) || (opts.descending && entry.seq <= since) {
            if !opts.descending { continue; }
        }
        if !opts.descending && entry.seq <= since { continue; }

        if let Some(ref doc_ids) = opts.doc_ids {
            if !doc_ids.contains(&entry.doc_id) { continue; }
        }

        // Get winning rev
        let doc_js = docs_store.get(wasm_bindgen::JsValue::from_str(&entry.doc_id)).map_err(idb_err)?.await.map_err(idb_err)?;
        let rev_str = doc_js.and_then(|v| serde_wasm_bindgen::from_value::<schema::StoredDoc>(v).ok())
            .and_then(|d| serde_json::from_str::<rouchdb_core::rev_tree::RevTree>(&d.rev_tree).ok())
            .and_then(|t| winning_rev(&t))
            .map(|r| r.to_string())
            .unwrap_or_default();

        results.push(ChangeEvent {
            seq: Seq::Num(entry.seq),
            id: entry.doc_id,
            changes: vec![ChangeRev { rev: rev_str }],
            deleted: entry.deleted,
            doc: None,
            conflicts: None,
        });

        if let Some(limit) = opts.limit {
            if results.len() >= limit as usize { break; }
        }
    }

    let last_seq = results.last().map(|r| r.seq.clone()).unwrap_or(opts.since.clone());
    Ok(ChangesResponse { results, last_seq })
}
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement changes()"
```

---

## Task 15: `revs_diff()` and `bulk_get()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
#[wasm_bindgen_test]
async fn revs_diff_and_bulk_get() {
    let db = IndexedDbAdapter::open("test-revs-diff").await.unwrap();
    let doc = Document {
        id: "doc1".into(), rev: None, deleted: false,
        data: serde_json::json!({"v": 1}),
        attachments: std::collections::HashMap::new(),
    };
    let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    let existing_rev = results[0].rev.clone().unwrap();

    let mut revs = std::collections::HashMap::new();
    revs.insert("doc1".into(), vec![existing_rev.clone(), "2-doesnotexist".into()]);
    revs.insert("doc2".into(), vec!["1-abc".into()]);

    let diff = db.revs_diff(revs).await.unwrap();
    assert!(!diff.results["doc1"].missing.contains(&existing_rev));
    assert!(diff.results["doc1"].missing.contains(&"2-doesnotexist".to_string()));
    assert!(diff.results["doc2"].missing.contains(&"1-abc".to_string()));

    // bulk_get
    let bg = db.bulk_get(vec![BulkGetItem { id: "doc1".into(), rev: None }]).await.unwrap();
    assert!(bg.results[0].docs[0].ok.is_some());
}
```

- [ ] **Step 2: Implement `revs_diff()`**

```rust
async fn revs_diff(&self, revs: std::collections::HashMap<String, Vec<String>>) -> Result<RevsDiffResponse> {
    use idb::TransactionMode;
    use rouchdb_core::rev_tree::{collect_leaves, rev_exists};

    let txn = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let docs_store = txn.object_store("docs").map_err(idb_err)?;
    let mut results = std::collections::HashMap::new();

    for (doc_id, rev_list) in revs {
        let stored: Option<schema::StoredDoc> = docs_store.get(wasm_bindgen::JsValue::from_str(&doc_id))
            .map_err(idb_err)?.await.map_err(idb_err)?
            .and_then(|v| serde_wasm_bindgen::from_value(v).ok());
        let tree: rouchdb_core::rev_tree::RevTree = stored.as_ref()
            .and_then(|d| serde_json::from_str(&d.rev_tree).ok())
            .unwrap_or_default();

        let mut missing = Vec::new();
        let mut possible_ancestors = Vec::new();
        for rev_str in &rev_list {
            let (pos, hash) = util::parse_rev(rev_str)?;
            if !rev_exists(&tree, pos, &hash) {
                missing.push(rev_str.clone());
                let leaves = collect_leaves(&tree);
                for leaf in &leaves {
                    if leaf.pos < pos { possible_ancestors.push(leaf.rev_string()); }
                }
            }
        }
        if !missing.is_empty() {
            results.insert(doc_id, RevsDiffResult { missing, possible_ancestors });
        }
    }
    Ok(RevsDiffResponse { results })
}
```

- [ ] **Step 3: Implement `bulk_get()`**

```rust
async fn bulk_get(&self, docs: Vec<BulkGetItem>) -> Result<BulkGetResponse> {
    use idb::TransactionMode;
    use rouchdb_core::merge::winning_rev;
    use rouchdb_core::rev_tree::find_rev_ancestry;

    let txn = self.db.transaction(&["docs", "revs"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let docs_store = txn.object_store("docs").map_err(idb_err)?;
    let revs_store = txn.object_store("revs").map_err(idb_err)?;
    let mut results = Vec::new();

    for item in docs {
        let stored_doc: Option<schema::StoredDoc> = docs_store.get(wasm_bindgen::JsValue::from_str(&item.id))
            .map_err(idb_err)?.await.map_err(idb_err)?
            .and_then(|v| serde_wasm_bindgen::from_value(v).ok());

        let mut bulk_docs = Vec::new();
        match stored_doc {
            None => {
                bulk_docs.push(BulkGetDoc { ok: None, error: Some(BulkGetError {
                    id: item.id.clone(), rev: item.rev.unwrap_or_default(),
                    error: "not_found".into(), reason: "missing".into(),
                })});
            }
            Some(ref stored) => {
                let tree: rouchdb_core::rev_tree::RevTree = serde_json::from_str(&stored.rev_tree).unwrap_or_default();
                let rev_str = if let Some(ref r) = item.rev { r.clone() }
                    else { match winning_rev(&tree) { Some(w) => w.to_string(), None => {
                        bulk_docs.push(BulkGetDoc { ok: None, error: Some(BulkGetError { id: item.id.clone(), rev: String::new(), error: "not_found".into(), reason: "missing".into() }) });
                        results.push(BulkGetResult { id: item.id, docs: bulk_docs });
                        continue;
                    }}};

                let rev_js = revs_store.get(wasm_bindgen::JsValue::from_str(&schema::rev_key(&item.id, &rev_str)))
                    .map_err(idb_err)?.await.map_err(idb_err)?;
                match rev_js {
                    None => bulk_docs.push(BulkGetDoc { ok: None, error: Some(BulkGetError { id: item.id.clone(), rev: rev_str, error: "not_found".into(), reason: "missing".into() }) }),
                    Some(v) => {
                        let sr: schema::StoredRev = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
                        let data: serde_json::Value = serde_json::from_str(&sr.data).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        let mut obj = match data { serde_json::Value::Object(m) => m, _ => serde_json::Map::new() };
                        obj.insert("_id".into(), serde_json::Value::String(item.id.clone()));
                        obj.insert("_rev".into(), serde_json::Value::String(rev_str.clone()));
                        if sr.deleted { obj.insert("_deleted".into(), serde_json::Value::Bool(true)); }
                        if let Ok((pos, ref hash)) = util::parse_rev(&rev_str) {
                            if let Some(ancestry) = find_rev_ancestry(&tree, pos, hash) {
                                obj.insert("_revisions".into(), serde_json::json!({ "start": pos, "ids": ancestry }));
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
```

- [ ] **Step 4: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement revs_diff() and bulk_get()"
```

---

## Task 16: `put_attachment()`, `get_attachment()`, `remove_attachment()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
#[wasm_bindgen_test]
async fn attachment_crud() {
    let db = IndexedDbAdapter::open("test-attach").await.unwrap();
    let doc = Document {
        id: "doc1".into(), rev: None, deleted: false,
        data: serde_json::json!({}),
        attachments: std::collections::HashMap::new(),
    };
    let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    let rev = results[0].rev.clone().unwrap();

    let data = b"hello world".to_vec();
    let result = db.put_attachment("doc1", "hello.txt", &rev, data.clone(), "text/plain").await.unwrap();
    assert!(result.ok);

    let fetched = db.get_attachment("doc1", "hello.txt", GetAttachmentOptions { rev: None }).await.unwrap();
    assert_eq!(fetched, data);
}
```

- [ ] **Step 2: Implement the three attachment methods**

```rust
async fn put_attachment(&self, doc_id: &str, att_id: &str, rev: &str, data: Vec<u8>, content_type: &str) -> Result<DocResult> {
    use idb::TransactionMode;
    use rouchdb_core::merge::winning_rev;
    use wasm_bindgen::JsValue;

    let digest = util::compute_attachment_digest(&data);
    let length = data.len() as u64;

    // Store attachment binary in `attachments` store (keyed by digest)
    let txn = self.db.transaction(&["attachments"], TransactionMode::ReadWrite).map_err(idb_err)?;
    let att_store = txn.object_store("attachments").map_err(idb_err)?;
    let arr = js_sys::Uint8Array::from(data.as_slice());
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(&obj, &"digest".into(), &JsValue::from_str(&digest)).map_err(js_err)?;
    js_sys::Reflect::set(&obj, &"data".into(), &arr).map_err(js_err)?;
    att_store.put(&obj.into(), Some(&JsValue::from_str(&digest))).map_err(idb_err)?.await.map_err(idb_err)?;
    txn.commit().map_err(idb_err)?.await.map_err(idb_err)?;

    // Get current doc and add attachment metadata, then write as new revision
    let doc = self.get(doc_id, GetOptions { rev: Some(rev.to_string()), ..Default::default() }).await?;
    let stored_winner = winning_rev({
        let txn2 = self.db.transaction(&["docs"], TransactionMode::ReadOnly).map_err(idb_err)?;
        let docs_store = txn2.object_store("docs").map_err(idb_err)?;
        let v = docs_store.get(JsValue::from_str(doc_id)).map_err(idb_err)?.await.map_err(idb_err)?
            .ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
        let sd: schema::StoredDoc = serde_wasm_bindgen::from_value(v).map_err(serde_err)?;
        serde_json::from_str(&sd.rev_tree).unwrap_or_default()
    }).ok_or_else(|| RouchError::NotFound(doc_id.to_string()))?;
    if stored_winner.to_string() != rev { return Err(RouchError::Conflict); }

    let att_meta = AttachmentMeta { content_type: content_type.to_string(), digest, length, stub: true, data: None };
    let new_doc = Document {
        id: doc_id.to_string(),
        rev: doc.rev,
        deleted: false,
        data: doc.data,
        attachments: { let mut m = HashMap::new(); m.insert(att_id.to_string(), att_meta); m },
    };
    Ok(self.process_doc_new_edits(new_doc).await)
}

async fn get_attachment(&self, doc_id: &str, att_id: &str, _opts: GetAttachmentOptions) -> Result<Vec<u8>> {
    use idb::TransactionMode;
    use wasm_bindgen::JsValue;

    // Get doc to find the attachment digest
    let doc = self.get(doc_id, GetOptions::default()).await?;
    let meta = doc.attachments.get(att_id)
        .ok_or_else(|| RouchError::NotFound(format!("{}/{}", doc_id, att_id)))?;
    let digest = meta.digest.clone();

    let txn = self.db.transaction(&["attachments"], TransactionMode::ReadOnly).map_err(idb_err)?;
    let att_store = txn.object_store("attachments").map_err(idb_err)?;
    let result = att_store.get(JsValue::from_str(&digest)).map_err(idb_err)?.await.map_err(idb_err)?
        .ok_or_else(|| RouchError::NotFound(format!("attachment data for digest {}", digest)))?;
    let data_val = js_sys::Reflect::get(&result, &"data".into()).map_err(js_err)?;
    let arr = js_sys::Uint8Array::from(data_val);
    Ok(arr.to_vec())
}

async fn remove_attachment(&self, doc_id: &str, _att_id: &str, rev: &str) -> Result<DocResult> {
    let doc = self.get(doc_id, GetOptions { rev: Some(rev.to_string()), ..Default::default() }).await?;
    let new_doc = Document {
        id: doc_id.to_string(), rev: doc.rev, deleted: false,
        data: doc.data, attachments: HashMap::new(),
    };
    Ok(self.process_doc_new_edits(new_doc).await)
}
```

- [ ] **Step 3: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement attachment operations"
```

---

## Task 17: `compact()` and `destroy()`

**Files:**
- Modify: `crates/rouchdb-adapter-indexeddb/src/lib.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[wasm_bindgen_test]
async fn compact_removes_old_revs() {
    let db = IndexedDbAdapter::open("test-compact").await.unwrap();
    let doc = Document { id: "d".into(), rev: None, deleted: false, data: serde_json::json!({"v":1}), attachments: std::collections::HashMap::new() };
    let r1 = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    let rev1: Revision = r1[0].rev.clone().unwrap().parse().unwrap();
    let doc2 = Document { id: "d".into(), rev: Some(rev1), deleted: false, data: serde_json::json!({"v":2}), attachments: std::collections::HashMap::new() };
    db.bulk_docs(vec![doc2], BulkDocsOptions::new()).await.unwrap();
    db.compact().await.unwrap();
    // After compact, getting current doc still works
    let fetched = db.get("d", GetOptions::default()).await.unwrap();
    assert_eq!(fetched.data["v"], 2);
}

#[wasm_bindgen_test]
async fn destroy_clears_all() {
    let db = IndexedDbAdapter::open("test-destroy").await.unwrap();
    let doc = Document { id: "d".into(), rev: None, deleted: false, data: serde_json::json!({}), attachments: std::collections::HashMap::new() };
    db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
    db.destroy().await.unwrap();
    let info = db.info().await.unwrap();
    assert_eq!(info.doc_count, 0);
    assert_eq!(info.update_seq, Seq::Num(0));
}
```

- [ ] **Step 2: Implement `compact()` and `destroy()`**

```rust
async fn compact(&self) -> Result<()> {
    use idb::TransactionMode;
    use rouchdb_core::rev_tree::collect_leaves;
    use wasm_bindgen::JsValue;

    let txn = self.db.transaction(&["docs", "revs"], TransactionMode::ReadWrite).map_err(idb_err)?;
    let docs_store = txn.object_store("docs").map_err(idb_err)?;
    let revs_store = txn.object_store("revs").map_err(idb_err)?;

    let all_docs = docs_store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?;
    for doc_val in all_docs {
        let stored: schema::StoredDoc = match serde_wasm_bindgen::from_value(doc_val) { Ok(d) => d, Err(_) => continue };
        let tree: rouchdb_core::rev_tree::RevTree = serde_json::from_str(&stored.rev_tree).unwrap_or_default();
        let leaves: std::collections::HashSet<String> = collect_leaves(&tree).iter().map(|l| l.rev_string()).collect();

        // Get all rev keys for this doc and delete non-leaf ones
        let all_rev_vals = revs_store.get_all(None, None).map_err(idb_err)?.await.map_err(idb_err)?;
        for rev_val in all_rev_vals {
            let sr: schema::StoredRev = match serde_wasm_bindgen::from_value(rev_val) { Ok(r) => r, Err(_) => continue };
            if sr.doc_id == stored.id && !leaves.contains(&sr.rev) {
                let key = JsValue::from_str(&schema::rev_key(&sr.doc_id, &sr.rev));
                let _ = revs_store.delete(key).map_err(idb_err)?.await;
            }
        }
    }
    txn.commit().map_err(idb_err)?.await.map_err(idb_err)?;
    Ok(())
}

async fn destroy(&self) -> Result<()> {
    use idb::TransactionMode;

    let stores = ["docs", "revs", "changes", "local_docs", "attachments", "meta"];
    let txn = self.db.transaction(&stores, TransactionMode::ReadWrite).map_err(idb_err)?;
    for name in &stores {
        let store = txn.object_store(name).map_err(idb_err)?;
        store.clear().map_err(idb_err)?.await.map_err(idb_err)?;
    }
    txn.commit().map_err(idb_err)?.await.map_err(idb_err)?;
    *self.update_seq.borrow_mut() = 0;
    Ok(())
}
```

- [ ] **Step 3: Run tests**

```bash
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rouchdb-adapter-indexeddb/src/lib.rs
git commit -m "feat(adapter-indexeddb): implement compact() and destroy()"
```

---

## Task 18: CI — add `test-wasm` job

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Append `test-wasm` job to CI workflow**

Add after the existing `test` job:

```yaml
  test-wasm:
    name: WASM Tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown

      - uses: Swatinem/rust-cache@v2

      - name: Install wasm-pack
        run: cargo install wasm-pack

      - name: Install Firefox (geckodriver)
        run: |
          sudo apt-get update
          sudo apt-get install -y firefox

      - name: Run WASM tests
        run: wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

- [ ] **Step 2: Verify it parses as valid YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo "valid YAML"
```

Expected: `valid YAML`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add test-wasm job for rouchdb-adapter-indexeddb"
```

---

## Self-Review Checklist

- [x] **Spec coverage:**
  - Section 1 (MaybeSend/MaybeSync): Tasks 1–2 ✓
  - Section 2 (IndexedDB schema, 6 stores): Task 7 ✓
  - Section 2 (IndexedDbAdapter crate): Task 6 ✓
  - Section 3 (reqwest conditional): Task 3 ✓
  - Section 3 (replicate_live gate): Task 4 ✓
  - Section 4 (workspace): Task 6 ✓
  - Section 4 (testing, 3 layers): Tasks 9–17 (unit/wasm-pack) + Task 18 (CI) ✓
  - All 6 object stores created in schema: Task 7 ✓
  - `put_attachment`/`get_attachment` use Uint8Array for binary: Task 16 ✓

- [x] **Type consistency:** `StoredDoc`, `StoredRev`, `StoredChange`, `StoredLocalDoc` defined in Task 7 and used identically in Tasks 9–17. `schema::rev_key()` helper defined in Task 7 and used in Tasks 10–16.

- [x] **No placeholders:** All methods have complete code.

- [x] **`idb` commit API:** Write transactions use `txn.commit().map_err(idb_err)?.await.map_err(idb_err)?`. Read transactions are left to auto-commit.
