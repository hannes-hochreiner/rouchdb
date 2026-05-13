# IndexedDB Storage Layer

The `rouchdb-adapter-indexeddb` crate provides browser-local storage for
WASM applications. It implements the `Adapter` trait using the browser's
built-in [IndexedDB](https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API)
API, accessed through the [`idb`](https://crates.io/crates/idb) crate.

This crate is **WASM-only** (`#[cfg(target_arch = "wasm32")]`). It never
compiles for native targets.

## Why IndexedDB

- **Browser-native persistence.** Data survives page reloads and browser
  restarts without any server round-trips.
- **Structured storage.** IndexedDB supports indexed queries and range scans,
  making it suitable for a document database with sequence-ordered changes.
- **Offline-first.** Combined with `rouchdb-adapter-http` and the replication
  protocol, applications can store data locally and sync with a CouchDB server
  whenever connectivity is available.

## Object Store Schema

The adapter opens a single IndexedDB database (named after the `name` argument
to `IndexedDbAdapter::open`) at schema version `1`. Six object stores are
created on first open:

```
+----------------+---------------------+-----------------------------+
| Object Store   | Key Path            | Indexes                     |
+----------------+---------------------+-----------------------------+
| docs           | id                  | (none)                      |
| revs           | key                 | by_doc_id (doc_id, unique=F)|
| changes        | seq                 | by_doc_id (doc_id, unique=F)|
| local_docs     | id                  | (none)                      |
| attachments    | digest              | (none)                      |
| meta           | key                 | (none)                      |
+----------------+---------------------+-----------------------------+
```

### `docs`

**Purpose:** Stores document metadata — the revision tree and the document's
current change sequence number.

**Key path:** `id` (the document ID string).

**Value type:** `StoredDoc` — serialized to a JS object via `serde-wasm-bindgen`:

```rust
struct StoredDoc {
    id: String,
    rev_tree: String, // JSON-encoded RevTree
    seq: u64,
}
```

The `rev_tree` field stores the complete revision tree as a JSON string rather
than a nested JS object. The rev tree is always read and written as a unit, so
splitting it into rows would add complexity without benefit. Storing it as a
string also avoids `serde-wasm-bindgen` edge cases with deeply nested structures
and keeps serialization deterministic.

### `revs`

**Purpose:** Stores the JSON body for each individual revision of a document.

**Key path:** `key` — a composite string `"{doc_id}\x00{rev}"`. The null byte
separator ensures all revisions for a given document sort contiguously.

**Indexes:**
- `by_doc_id` (field: `doc_id`, `unique: false`) — groups revisions by document
  ID. Created for future compaction optimisations; the current `compact()`
  implementation uses `get_all` and filters in Rust (see [Compaction](#compaction)).

**Value type:** `StoredRev`:

```rust
struct StoredRev {
    key: String,   // "<doc_id>\x00<rev>"
    doc_id: String,
    rev: String,
    data: String,  // JSON-encoded serde_json::Value
    deleted: bool,
}
```

The `data` field stores the document body as a JSON string. CouchDB underscore
fields (`_id`, `_rev`, `_deleted`, `_attachments`, `_revisions`) are stripped
before storage and re-injected on read.

### `changes`

**Purpose:** Implements the changes feed. Each entry records the most recent
write for a given document.

**Key path:** `seq` (a `u64` sequence number).

**Indexes:**
- `by_doc_id` (field: `doc_id`, `unique: false`) — groups change entries by
  document ID. Created for future use; the current implementation deletes old
  change entries by their `seq` key directly (the existing sequence number is
  read from `StoredDoc.seq` before the write transaction opens).

**Value type:** `StoredChange`:

```rust
struct StoredChange {
    seq: u64,
    doc_id: String,
    deleted: bool,
}
```

When a document is written, the adapter:

1. Reads the document's previous `seq` from its `StoredDoc` entry (if any).
2. Deletes the old `changes` entry at that sequence number.
3. Inserts a new entry at the new (incremented) sequence number.

This means each document appears at most once in the changes store, at its
latest sequence. Querying `changes(since: N)` performs a key range scan
over `(N, +∞)`.

**Example state after 5 writes:**

```
seq | doc_id  | deleted
----|---------|--------
  3 | "doc1"  | false      (updated at seq 1, again at seq 3)
  4 | "doc2"  | false
  5 | "doc3"  | true       (deleted)
```

### `local_docs`

**Purpose:** Non-replicated local documents. The replication protocol uses
these to store checkpoints (`_local/{replication-id}`).

**Key path:** `id` (the local document ID, without the `_local/` prefix).

**Value type:** `StoredLocalDoc`:

```rust
struct StoredLocalDoc {
    id: String,
    data: String, // JSON-encoded serde_json::Value
}
```

Local documents have no revision tree and do not appear in the changes feed or
`_all_docs` results.

### `attachments`

**Purpose:** Stores raw binary attachment data, keyed by content digest.

**Key path:** `digest` — a content-based identifier (e.g., `"md5-abc123..."`).

Content-addressable storage means identical attachment bytes are stored only
once regardless of how many revisions reference them.

> **Note:** Attachment metadata (content-type, length) is stored inline in the
> document body as a JSON `_attachments` field. The `attachments` object store
> holds only the raw bytes.

### `meta`

**Purpose:** Global database metadata.

**Key path:** `key` (string).

One entry is stored:

| key          | value                                 |
|--------------|---------------------------------------|
| `update_seq` | `{ key: "update_seq", value: "<n>" }` |

The `update_seq` value is stored as a string (not a JS number) to avoid
precision loss for large 64-bit integers in JavaScript. The database name is
kept only in the `IndexedDbAdapter` struct field — it is not persisted to
IndexedDB because it can always be recovered from the `IDBDatabase.name`
property.

## Adapter Struct

```rust
pub struct IndexedDbAdapter {
    db: idb::Database,      // handle to the open IndexedDB database
    name: String,           // logical database name
    update_seq: Cell<u64>,  // in-memory sequence counter (no lock needed)
}
```

## Sequence Counter

The adapter keeps an in-memory `Cell<u64>` for the current `update_seq`. This
is safe because the browser JavaScript runtime is single-threaded — there is no
concurrent access and no `Mutex` or `RwLock` is needed. The counter is loaded
from the `meta` store when `IndexedDbAdapter::open()` is called and is written
back to the `meta` store on every document write.

The counter is advanced **only after** a successful write: if the IndexedDB
write transaction fails, the `Cell` is not updated and the sequence remains
consistent with the store.

## Write Ordering and IndexedDB Transactions

IndexedDB write transactions auto-commit when no requests are pending at the
end of a microtask. This means **all `put`/`delete` requests must be queued
synchronously** before any `.await` point within a transaction.

The adapter follows this rule throughout:

- **Writes** (`write_doc`): all five requests — `docs` put, `revs` put, old
  `changes` delete, new `changes` put, `meta` put — are queued synchronously
  in a single read-write transaction before any `await`.
- **Batch reads** (`batch_load_rev_data`): all `get` requests for a set of
  `(doc_id, rev)` pairs are queued synchronously in a single read-only
  transaction before awaiting results. This is used by `all_docs` and
  `changes` when `include_docs=true`.

## Two Write Modes

### `new_edits=true` (Normal Writes)

Used for local application writes:

1. If the document already exists, the provided `_rev` must match the current
   winning revision — otherwise a conflict error is returned.
2. A new revision hash is generated: `MD5(prev_rev + deleted_flag + json_body)`.
3. The new revision is merged into the existing revision tree.

### `new_edits=false` (Replication Writes)

Used during replication from another adapter:

1. No conflict check is performed.
2. The revision ID from the incoming document is accepted as-is.
3. If `_revisions` metadata is present, the full ancestry path is reconstructed
   using `build_path_from_revs` and merged into the tree.
4. `_revisions` is stripped from the stored document body.

## Compaction

`compact()` reclaims storage by deleting revision bodies for non-leaf
revisions. It runs in three phases, each in its own transaction:

1. **Load** all `StoredDoc` records (read-only) and compute the set of leaf
   revision strings for every document using `collect_leaves()`.
2. **Load** all `StoredRev` records (read-only) and collect the `key` values
   of entries whose `rev` is not in their document's leaf set.
3. **Delete** all non-leaf `StoredRev` keys in a single read-write transaction,
   with all delete requests queued synchronously before any `await`.

The revision tree in `docs` is not modified — leaf determination always uses
the live tree. After compaction, requesting a non-leaf revision body (e.g.,
via `get()` with an explicit `rev`) will return an empty document body because
its `StoredRev` entry no longer exists.

## Destroy

`destroy()` clears all six object stores sequentially and resets the in-memory
`update_seq` counter to `0`. The IndexedDB database object itself remains open
and valid — the adapter can continue to be used after destruction (the next
write will start from sequence `1`).

## Key Format Summary

```
docs:        "doc1"                   -> StoredDoc { rev_tree: "...", seq: 3 }
revs:        "doc1\x003-a1b2c3..."   -> StoredRev { data: "{...}", deleted: false }
changes:     3                        -> StoredChange { doc_id: "doc1", deleted: false }
local_docs:  "replication-id-hash"   -> StoredLocalDoc { data: "{...}" }
attachments: "md5-abcdef..."         -> { digest: "...", data: <bytes> }
meta:        "update_seq"            -> { key: "update_seq", value: "3" }
```
