#![cfg(target_arch = "wasm32")]

mod schema;
mod util;

pub use schema::IndexedDbAdapter;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::IndexedDbAdapter;
    use rouchdb_core::adapter::Adapter;
    use rouchdb_core::document::*;
    use rouchdb_core::error::RouchError;
    use wasm_bindgen_test::wasm_bindgen_test;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    // -------------------------------------------------------------------------
    // Task 9 — info(), get_local(), put_local(), remove_local()
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn info_empty_db() {
        let db = IndexedDbAdapter::open("t9-info").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t9-info").await.unwrap();
        let info = db.info().await.unwrap();
        assert_eq!(info.db_name, "t9-info");
        assert_eq!(info.doc_count, 0);
        assert_eq!(info.update_seq, Seq::Num(0));
    }

    #[wasm_bindgen_test]
    async fn local_docs_crud() {
        let db = IndexedDbAdapter::open("t9-local").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t9-local").await.unwrap();

        let value = serde_json::json!({"checkpoint": 42});
        db.put_local("repl-id", value.clone()).await.unwrap();

        let fetched = db.get_local("repl-id").await.unwrap();
        assert_eq!(fetched["checkpoint"], 42);

        db.remove_local("repl-id").await.unwrap();

        let result = db.get_local("repl-id").await;
        assert!(matches!(result.unwrap_err(), RouchError::NotFound(_)));
    }

    // -------------------------------------------------------------------------
    // Task 10 — bulk_docs() new_edits=true
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn create_document() {
        let db = IndexedDbAdapter::open("t10-create").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t10-create").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };

        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        assert!(results[0].ok);
        assert_eq!(results[0].id, "doc1");
        assert!(results[0].rev.is_some());
    }

    #[wasm_bindgen_test]
    async fn update_document() {
        let db = IndexedDbAdapter::open("t10-update").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t10-update").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        let rev1_str = results[0].rev.clone().unwrap();
        let rev1: Revision = rev1_str.parse().unwrap();

        let doc2 = Document {
            id: "doc1".into(),
            rev: Some(rev1),
            deleted: false,
            data: serde_json::json!({"name": "Bob"}),
            attachments: HashMap::new(),
        };
        let results2 = db.bulk_docs(vec![doc2], BulkDocsOptions::new()).await.unwrap();
        assert!(results2[0].ok);
        let rev2 = results2[0].rev.clone().unwrap();
        assert!(rev2.starts_with("2-"), "expected rev starting with '2-', got {}", rev2);

        let fetched = db.get("doc1", GetOptions::default()).await.unwrap();
        assert_eq!(fetched.data["name"], "Bob");
    }

    #[wasm_bindgen_test]
    async fn conflict_on_wrong_rev() {
        let db = IndexedDbAdapter::open("t10-conflict").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t10-conflict").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"v": 1}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();

        let doc2 = Document {
            id: "doc1".into(),
            rev: Some(Revision::new(1, "wronghash".into())),
            deleted: false,
            data: serde_json::json!({"v": 2}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc2], BulkDocsOptions::new()).await.unwrap();
        assert!(!results[0].ok);
        assert_eq!(results[0].error.as_deref(), Some("conflict"));
    }

    #[wasm_bindgen_test]
    async fn delete_document() {
        let db = IndexedDbAdapter::open("t10-delete").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t10-delete").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        let rev1: Revision = results[0].rev.clone().unwrap().parse().unwrap();

        let del = Document {
            id: "doc1".into(),
            rev: Some(rev1),
            deleted: true,
            data: serde_json::json!({}),
            attachments: HashMap::new(),
        };
        let results2 = db.bulk_docs(vec![del], BulkDocsOptions::new()).await.unwrap();
        assert!(results2[0].ok);

        let info = db.info().await.unwrap();
        assert_eq!(info.doc_count, 0);

        let get_result = db.get("doc1", GetOptions::default()).await;
        assert!(matches!(get_result.unwrap_err(), rouchdb_core::error::RouchError::NotFound(_)));
    }

    #[wasm_bindgen_test]
    async fn auto_generate_id() {
        let db = IndexedDbAdapter::open("t10-autoid").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t10-autoid").await.unwrap();

        let doc = Document {
            id: String::new(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "no-id"}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        assert!(results[0].ok);
        assert!(!results[0].id.is_empty());
    }

    // -------------------------------------------------------------------------
    // Task 11 — bulk_docs() new_edits=false (replication mode)
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn replication_mode_bulk_docs() {
        let db = IndexedDbAdapter::open("t11-repl").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t11-repl").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: Some(Revision::new(1, "abc123".into())),
            deleted: false,
            data: serde_json::json!({"name": "replicated"}),
            attachments: HashMap::new(),
        };

        let results = db
            .bulk_docs(vec![doc], BulkDocsOptions::replication())
            .await
            .unwrap();
        assert!(results[0].ok);
        assert_eq!(results[0].rev.as_deref(), Some("1-abc123"));
    }

    #[wasm_bindgen_test]
    async fn replication_missing_rev() {
        let db = IndexedDbAdapter::open("t11-repl-norev").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t11-repl-norev").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "no-rev"}),
            attachments: HashMap::new(),
        };

        let results = db
            .bulk_docs(vec![doc], BulkDocsOptions::replication())
            .await
            .unwrap();
        assert!(!results[0].ok);
    }

    // -------------------------------------------------------------------------
    // Task 12 — get()
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn get_existing_doc() {
        let db = IndexedDbAdapter::open("t12-get").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t12-get").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();

        let fetched = db.get("doc1", GetOptions::default()).await.unwrap();
        assert_eq!(fetched.id, "doc1");
        assert_eq!(fetched.data["name"], "Alice");
        assert!(fetched.rev.is_some());
    }

    #[wasm_bindgen_test]
    async fn get_nonexistent_doc() {
        let db = IndexedDbAdapter::open("t12-get-missing").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t12-get-missing").await.unwrap();

        let result = db.get("does-not-exist", GetOptions::default()).await;
        assert!(matches!(result.unwrap_err(), RouchError::NotFound(_)));
    }

    #[wasm_bindgen_test]
    async fn get_deleted_doc() {
        let db = IndexedDbAdapter::open("t12-get-deleted").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t12-get-deleted").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        let rev1: Revision = results[0].rev.clone().unwrap().parse().unwrap();

        let del = Document {
            id: "doc1".into(),
            rev: Some(rev1),
            deleted: true,
            data: serde_json::json!({}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![del], BulkDocsOptions::new()).await.unwrap();

        let result = db.get("doc1", GetOptions::default()).await;
        assert!(matches!(result.unwrap_err(), RouchError::NotFound(_)));
    }

    // -------------------------------------------------------------------------
    // Task 13 — all_docs()
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn all_docs_sorted_keys() {
        let db = IndexedDbAdapter::open("t13-sorted").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t13-sorted").await.unwrap();

        for name in ["charlie", "alice", "bob"] {
            let doc = Document {
                id: name.into(),
                rev: None,
                deleted: false,
                data: serde_json::json!({"name": name}),
                attachments: HashMap::new(),
            };
            db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        }

        let result = db.all_docs(AllDocsOptions::new()).await.unwrap();
        assert_eq!(result.total_rows, 3);
        assert_eq!(result.rows[0].id, "alice");
        assert_eq!(result.rows[1].id, "bob");
        assert_eq!(result.rows[2].id, "charlie");
    }

    #[wasm_bindgen_test]
    async fn all_docs_include_docs() {
        let db = IndexedDbAdapter::open("t13-incdocs").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t13-incdocs").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();

        let mut opts = AllDocsOptions::new();
        opts.include_docs = true;
        let result = db.all_docs(opts).await.unwrap();
        assert!(result.rows[0].doc.is_some());
        let fetched = result.rows[0].doc.as_ref().unwrap();
        assert_eq!(fetched["name"], "Alice");
    }

    #[wasm_bindgen_test]
    async fn all_docs_excludes_deleted() {
        let db = IndexedDbAdapter::open("t13-excldeleted").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t13-excldeleted").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "Alice"}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        let rev1: Revision = results[0].rev.clone().unwrap().parse().unwrap();

        let del = Document {
            id: "doc1".into(),
            rev: Some(rev1),
            deleted: true,
            data: serde_json::json!({}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![del], BulkDocsOptions::new()).await.unwrap();

        let result = db.all_docs(AllDocsOptions::new()).await.unwrap();
        assert_eq!(result.total_rows, 0);
        assert!(result.rows.is_empty());
    }

    // -------------------------------------------------------------------------
    // Task 14 — changes()
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn changes_all() {
        let db = IndexedDbAdapter::open("t14-all").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t14-all").await.unwrap();

        for i in 0..3u32 {
            let doc = Document {
                id: format!("doc{}", i),
                rev: None,
                deleted: false,
                data: serde_json::json!({"i": i}),
                attachments: HashMap::new(),
            };
            db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        }

        let changes = db.changes(ChangesOptions::default()).await.unwrap();
        assert_eq!(changes.results.len(), 3);
        let ids: Vec<&str> = changes.results.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"doc0"));
        assert!(ids.contains(&"doc1"));
        assert!(ids.contains(&"doc2"));
    }

    #[wasm_bindgen_test]
    async fn changes_since() {
        let db = IndexedDbAdapter::open("t14-since").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t14-since").await.unwrap();

        for i in 0..3u32 {
            let doc = Document {
                id: format!("doc{}", i),
                rev: None,
                deleted: false,
                data: serde_json::json!({"i": i}),
                attachments: HashMap::new(),
            };
            db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        }

        let changes = db
            .changes(ChangesOptions {
                since: Seq::Num(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(changes.results.len(), 1);
        assert_eq!(changes.results[0].id, "doc2");
    }

    #[wasm_bindgen_test]
    async fn changes_include_docs() {
        let db = IndexedDbAdapter::open("t14-incdocs").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t14-incdocs").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"x": 1}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();

        let changes = db
            .changes(ChangesOptions {
                include_docs: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(changes.results[0].doc.is_some());
        let fetched = changes.results[0].doc.as_ref().unwrap();
        assert_eq!(fetched["x"], 1);
    }

    // -------------------------------------------------------------------------
    // Task 15 — revs_diff() and bulk_get()
    // -------------------------------------------------------------------------

    #[wasm_bindgen_test]
    async fn revs_diff_missing_and_present() {
        let db = IndexedDbAdapter::open("t15-revsdiff").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t15-revsdiff").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"v": 1}),
            attachments: HashMap::new(),
        };
        let results = db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();
        let existing_rev = results[0].rev.clone().unwrap();

        let mut revs = HashMap::new();
        revs.insert(
            "doc1".into(),
            vec![existing_rev.clone(), "2-doesnotexist".into()],
        );

        let diff = db.revs_diff(revs).await.unwrap();
        let doc1_diff = diff.results.get("doc1").unwrap();
        assert!(!doc1_diff.missing.contains(&existing_rev));
        assert!(doc1_diff.missing.contains(&"2-doesnotexist".to_string()));
    }

    #[wasm_bindgen_test]
    async fn revs_diff_completely_missing_doc() {
        let db = IndexedDbAdapter::open("t15-revsdiff-ghost").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t15-revsdiff-ghost").await.unwrap();

        let mut revs = HashMap::new();
        revs.insert("ghost".into(), vec!["1-abc".into()]);

        let diff = db.revs_diff(revs).await.unwrap();
        let ghost_diff = diff.results.get("ghost").unwrap();
        assert!(ghost_diff.missing.contains(&"1-abc".to_string()));
    }

    #[wasm_bindgen_test]
    async fn bulk_get_existing() {
        let db = IndexedDbAdapter::open("t15-bulkget").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t15-bulkget").await.unwrap();

        let doc = Document {
            id: "doc1".into(),
            rev: None,
            deleted: false,
            data: serde_json::json!({"name": "test"}),
            attachments: HashMap::new(),
        };
        db.bulk_docs(vec![doc], BulkDocsOptions::new()).await.unwrap();

        let result = db
            .bulk_get(vec![BulkGetItem {
                id: "doc1".into(),
                rev: None,
            }])
            .await
            .unwrap();

        let ok_doc = result.results[0].docs[0].ok.as_ref().unwrap();
        assert_eq!(result.results[0].id, "doc1");
        assert_eq!(ok_doc["_id"], "doc1");
        assert_eq!(ok_doc["name"], "test");
    }

    #[wasm_bindgen_test]
    async fn bulk_get_missing() {
        let db = IndexedDbAdapter::open("t15-bulkget-miss").await.unwrap();
        db.destroy().await.unwrap();
        let db = IndexedDbAdapter::open("t15-bulkget-miss").await.unwrap();

        let result = db
            .bulk_get(vec![BulkGetItem {
                id: "missing".into(),
                rev: None,
            }])
            .await
            .unwrap();

        assert!(result.results[0].docs[0].error.is_some());
    }
}
