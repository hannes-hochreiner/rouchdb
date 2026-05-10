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

The `rev_tree` field stores the complete revision tree as a JSON string (not a
nested JS object), keeping serialization deterministic and avoiding
`serde-wasm-bindgen` edge cases with deeply nested structures.

### `revs`

**Purpose:** Stores the JSON body for each individual revision of a document.

**Key path:** `key` — a composite string `"{doc_id}\x00{rev}"`. The null byte
separator ensures all revisions for a given document sort contiguously.

**Indexes:**
- `by_doc_id` (field: `doc_id`, `unique: false`) — allows fetching all
  revisions for a given document ID during compaction and `bulk_get`.

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
- `by_doc_id` (field: `doc_id`, `unique: false`) — allows removing the old
  change entry when a document is updated (a document may appear at multiple
  sequence positions during its lifetime, but only the latest is kept in the
  final feed).

**Value type:** `StoredChange`:

```rust
struct StoredChange {
    seq: u64,
    doc_id: String,
    deleted: bool,
}
```

When a document is written, the adapter:

1. Queries `by_doc_id` to find and delete any existing change entry for this
   document ID.
2. Inserts a new entry at the new (incremented) sequence number.

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

Two entries are stored:

| key          | value                                 |
|--------------|---------------------------------------|
| `update_seq` | `{ key: "update_seq", value: "<n>" }` |
| `db_uuid`    | `{ key: "db_uuid", value: "<uuid>" }` |

The `update_seq` value is stored as a string (not a JS number) to avoid
precision loss for large 64-bit integers in JavaScript.

## Sequence Counter

The adapter keeps an in-memory `Cell<u64>` for the current `update_seq`. This
is safe because the browser JavaScript runtime is single-threaded — there is no
concurrent access. The counter is loaded from the `meta` store when
`IndexedDbAdapter::open()` is called and is written back to the store on every
document write.

The counter is advanced **only after** a successful write: if the IndexedDB
write transaction fails, the `Cell` is not updated and the sequence remains
consistent with the store.

## Write Ordering and IndexedDB Transactions

IndexedDB write transactions auto-commit when no requests are pending at the
end of a microtask. This means **all `put`/`delete` requests must be queued
synchronously** before any `.await` point within a transaction.

The adapter follows this rule throughout: each write method (e.g., writing to
`docs`, `revs`, `changes`, and `meta` in a single `bulk_docs` call) stages all
requests in a single transaction before awaiting any of them.

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

## Destroy

`destroy()` clears all object stores and resets `update_seq` to `0` with a
fresh UUID. The IndexedDB database itself remains open and usable — the adapter
can continue to be used after destruction.

## Key Format Summary

```
docs:        "doc1"                   -> StoredDoc { rev_tree: "...", seq: 3 }
revs:        "doc1\x003-a1b2c3..."   -> StoredRev { data: "{...}", deleted: false }
changes:     3                        -> StoredChange { doc_id: "doc1", deleted: false }
local_docs:  "replication-id-hash"   -> StoredLocalDoc { data: "{...}" }
attachments: "md5-abcdef..."         -> { digest: "...", data: <bytes> }
meta:        "update_seq"            -> { key: "update_seq", value: "3" }
```
