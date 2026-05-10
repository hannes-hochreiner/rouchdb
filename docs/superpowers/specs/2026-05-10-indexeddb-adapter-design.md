# IndexedDB Adapter Design

**Date:** 2026-05-10
**Branch:** `feature/adapter-indexeddb`
**Goal:** Add a `rouchdb-adapter-indexeddb` crate so that Rust WASM applications (Leptos, Yew, Dioxus) can use RouchDB with persistent local storage in the browser, replicating bidirectionally with CouchDB — a Rust replacement for PouchDB.

---

## Scope

**In scope:**
- New crate `rouchdb-adapter-indexeddb` (WASM-only, `wasm32-unknown-unknown`)
- Modify `rouchdb-core`: make `Adapter` trait `?Send`-compatible on WASM
- Modify `rouchdb`: make `Plugin` trait and `Database` struct WASM-compatible
- Modify `rouchdb-replication`: replace Tokio-specific APIs with WASM-compatible alternatives
- Modify `rouchdb-adapter-http`: conditional `reqwest` features for WASM
- CI job for `wasm-pack test`

**Out of scope:**
- JavaScript/TypeScript bindings (`wasm-bindgen` public surface) — use PouchDB for JS consumers
- `rouchdb-views` / `rouchdb-query` WASM compatibility (follow-up)
- `rouchdb-server` on WASM (not meaningful in a browser)
- PouchDB-compatible on-disk schema (no requirement for cross-tool data sharing)

---

## Section 1: `rouchdb-core` — WASM-compatible `Adapter` trait

### Problem

`async_trait` generates futures that are `+ Send`, and the trait bound `: Send + Sync` means no `JsValue`-backed type can implement `Adapter` on WASM.

### Solution: `MaybeSend` / `MaybeSync` marker traits

Add two marker traits to `rouchdb-core/src/lib.rs`:

```rust
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

### `Adapter` trait change

```rust
// Before
#[async_trait]
pub trait Adapter: Send + Sync { ... }

// After
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Adapter: MaybeSend + MaybeSync { ... }
```

All method signatures are unchanged. On native targets, the `MaybeSend`/`MaybeSync` blanket impls preserve the existing `Send + Sync` requirement. On WASM, the bounds dissolve.

### `Plugin` trait change (`rouchdb/src/lib.rs`)

Same pattern applied to the `Plugin` trait and the `Database` struct's `Arc<dyn Adapter>` field (which remains `Arc` on both targets; on WASM it simply won't be `Send`, which is acceptable in a single-threaded environment).

---

## Section 2: IndexedDB Schema — Normalized Per-Revision Stores

One IndexedDB database per rouchdb database. The database name is passed to `IndexedDbAdapter::open(name)`. Six object stores are created on `onupgradeneeded`:

### Object stores

| Store | Key | Fields |
|---|---|---|
| `docs` | `id: String` | `id`, `rev_tree: String (JSON)`, `seq: u64` |
| `revs` | `"<doc_id>\x00<rev_string>"` | `doc_id`, `rev`, `data: String (JSON)`, `deleted: bool` |
| `changes` | `seq: u64` | `seq`, `doc_id`, `deleted: bool` |
| `local_docs` | `id: String` | `id`, `data: String (JSON)` |
| `attachments` | `digest: String` | `digest`, `data: Uint8Array` |
| `meta` | `key: String` | `key`, `value: String` |

### Design decisions

- **Rev tree as JSON blob in `docs`**: The rev tree is always read and written as a unit. Splitting it into rows would add complexity without benefit. It remains a serialized JSON string.
- **Rev bodies in `revs`**: Each revision body is a separate record keyed by `"<doc_id>\x00<rev_string>"`. `get()` fetches only the winning revision's body. `compact()` deletes all non-leaf `revs` entries for each document.
- **`changes` store**: Mirrors `MemoryAdapter`'s `BTreeMap<u64, (String, bool)>`. On each document write, the old `changes` entry for that doc is deleted and a new one at the new `seq` is inserted — preserving the "one entry per doc" invariant.
- **`meta` store**: Persists `update_seq` (as a string) and `db_name`. `update_seq` is read once at open and cached in-memory as a `u64` to avoid a store read on every write.
- **Attachments by digest**: Stored by content-addressed digest, so identical attachments are deduplicated automatically (same behaviour as `MemoryAdapter`).

### New crate: `rouchdb-adapter-indexeddb`

**Location:** `crates/rouchdb-adapter-indexeddb/`

**`Cargo.toml` dependencies:**
```toml
[dependencies]
rouchdb-core = { path = "../rouchdb-core", version = "0.3.2" }
async-trait = "0.1"
base64 = "0.22"
md-5 = "0.10"
serde_json = "1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
idb = "0.6"
js-sys = "0.3"
uuid = { version = "1", features = ["v4", "js"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
wasm-bindgen-test = "0.3"
```

The entire crate is gated `#![cfg(target_arch = "wasm32")]`. WASM-specific deps are target-gated, so `cargo build --workspace` on native compiles the crate as an empty library with no errors and no symbols — no `default-members` change is needed in the root workspace `Cargo.toml`.

**Struct:**
```rust
pub struct IndexedDbAdapter {
    db: idb::Database,
    name: String,
    update_seq: std::cell::Cell<u64>,  // single-threaded, no lock needed
}
```

---

## Section 3: WASM-compatibility for `rouchdb-adapter-http` and `rouchdb-replication`

### `rouchdb-adapter-http`

`reqwest` 0.12 supports `wasm32-unknown-unknown` via the browser Fetch API. The `cookies` feature is unavailable on WASM (the browser's native cookie jar handles `Set-Cookie` automatically). Feature selection becomes target-conditional:

```toml
[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json"] }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
reqwest = { version = "0.12", features = ["cookies", "rustls-tls"] }
```

No changes to `HttpAdapter` logic — the Fetch API behaves identically from Rust's perspective.

### `rouchdb-replication`

Two Tokio APIs that do not compile on WASM are replaced with cfg-gated helpers:

**1. Retry sleep:**
```rust
#[cfg(not(target_arch = "wasm32"))]
async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}
#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u64) {
    gloo_timers::future::sleep(std::time::Duration::from_millis(ms)).await;
}
```

**2. Background task spawn:**
```rust
#[cfg(not(target_arch = "wasm32"))]
macro_rules! spawn { ($f:expr) => { tokio::spawn($f) }; }
#[cfg(target_arch = "wasm32")]
macro_rules! spawn { ($f:expr) => { wasm_bindgen_futures::spawn_local($f) }; }
```

**New WASM-only dependencies in `rouchdb-replication/Cargo.toml`:**
```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
gloo-timers = { version = "0.3", features = ["futures"] }
wasm-bindgen-futures = "0.4"
```

`tokio::sync` (channels, `RwLock`, `Mutex`) compiles fine on WASM — no changes needed there.

---

## Section 4: Workspace Integration, Testing & CI

### Workspace

`rouchdb-adapter-indexeddb` is added to `workspace.members` in the root `Cargo.toml`. No `default-members` change is needed: because all WASM-specific deps are target-gated and the crate body is `#![cfg(target_arch = "wasm32")]`, native builds compile it as an empty library with no errors.

```toml
[workspace]
members = [
    # ... existing members ...
    "crates/rouchdb-adapter-indexeddb",
]
```

### Testing strategy

**Layer 1 — Unit tests** (`crates/rouchdb-adapter-indexeddb/src/lib.rs`):
Run per-method tests against real browser IndexedDB using `wasm-bindgen-test`. Each test is annotated `#[wasm_bindgen_test]`. Run with:
```
wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

**Layer 2 — Parity tests** (`crates/rouchdb/tests/parity_indexeddb.rs`):
Mirror the existing `parity_core.rs` suite, running the same behavioural assertions against `IndexedDbAdapter`. Ensures it is a drop-in replacement for `MemoryAdapter`.

**Layer 3 — Replication integration tests**:
Test full CouchDB sync from WASM against a live CouchDB instance, gated with `#[ignore]` and `COUCHDB_URL`, matching existing integration test conventions.

### CI

New job in `.github/workflows/ci.yml`:
```yaml
test-wasm:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with:
        targets: wasm32-unknown-unknown
    - run: cargo install wasm-pack
    - run: wasm-pack test --headless --firefox crates/rouchdb-adapter-indexeddb
```

### Publishing order

`rouchdb-adapter-indexeddb` depends only on `rouchdb-core`, so it publishes immediately after `rouchdb-core` and before the umbrella `rouchdb` crate.

---

## Implementation sequence

1. Modify `rouchdb-core`: add `MaybeSend`/`MaybeSync`, update `Adapter` trait
2. Modify `rouchdb`: update `Plugin` trait and `Database` struct
3. Modify `rouchdb-adapter-http`: conditional `reqwest` features
4. Modify `rouchdb-replication`: WASM-compatible sleep and spawn
5. Create `rouchdb-adapter-indexeddb`: crate scaffold, schema setup, `Adapter` impl
6. Write parity tests
7. Add CI job
