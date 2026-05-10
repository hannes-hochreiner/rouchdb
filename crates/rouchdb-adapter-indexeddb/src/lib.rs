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
}
