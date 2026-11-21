//! UI-independent Rust application core. No listener or Python delegation.
pub mod admission;
pub mod jobs;
pub mod matters;
pub mod planner;
mod request_order;
pub mod secure_db;
pub mod vault;
pub mod vault_store;
pub use request_order::RequestOrder;
use serde::Serialize;
use serde_json::{json, Value};

pub const RUST_WORKFLOW_READY: bool = false;

#[derive(Debug, Serialize)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}
fn reply(status: u16, body: Value) -> Reply {
    Reply { status, body }
}
pub fn import_timestamp(milliseconds: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(milliseconds).map(|stamp| stamp.to_rfc3339())
}
pub fn unsupported() -> Reply {
    reply(
        501,
        json!({"detail":"Not yet implemented in Rust development mode. Use the Python launcher for the full workflow."}),
    )
}
/// Temporary compatibility adapter for existing UI read contracts, not an HTTP router.
/// It never accepts filesystem paths, opens Python data, or silently delegates.
pub fn request(method: &str, path: &str) -> Reply {
    if method != "GET" {
        return unsupported();
    }
    match path {
        "/health" => reply(
            200,
            json!({"status":"ok","version":"0.1.0","app":"MatterDraft Studio","ocr_engine":"PDF Oxide and local Tesseract","backend":"rust-ipc","workflow_ready":RUST_WORKFLOW_READY}),
        ),
        "/capabilities" => reply(
            200,
            json!({"backend":"rust-ipc","writes":true,"matter_crud":true,"storage":"isolated-plaintext-synthetic","history_replay":true,"history_replay_scope":"matter-preparation-document-presence-and-order","matter_archive_restore":true,"vault":false,"workflow_ready":RUST_WORKFLOW_READY}),
        ),
        "/vault/status" => reply(200, json!({"state":"unlocked","enabled":false})),

        "/profiles" => reply(
            200,
            serde_json::from_str(include_str!("../../backend/app/rules/profiles.json"))
                .expect("embedded profiles"),
        ),
        _ => unsupported(),
    }
}
/// Bounded extraction primitive for synthetic development checks; no file writes.
pub fn extract_text(bytes: Vec<u8>) -> Result<Value, &'static str> {
    const ERROR: &str = "PDF extraction failed or exceeds supported limits";
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(ERROR);
    }
    let pdf = pdf_oxide::PdfDocument::from_bytes(bytes).map_err(|_| ERROR)?;
    if pdf.is_encrypted() {
        return Err(ERROR);
    }
    let count = pdf.page_count().map_err(|_| ERROR)?;
    if count > 2000 {
        return Err(ERROR);
    }
    let mut pages = Vec::new();
    let mut total = 0;
    for i in 0..count {
        let text = pdf.extract_text(i).map_err(|_| ERROR)?;
        total += text.len();
        if total > 10_000_000 {
            return Err(ERROR);
        }
        pages.push(text);
    }
    Ok(json!({"engine":"pdf_oxide","version":"0.3.78","pages":pages}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_and_refusals() {
        assert_eq!(request("GET", "/matters").status, 501);
        assert_eq!(request("GET", "/health").body["workflow_ready"], false);
        for (method, path) in [
            ("POST", "/matters"),
            ("GET", "/etc/passwd"),
            ("GET", "/documents/unknown/pdf"),
        ] {
            assert_eq!(request(method, path).status, 501);
        }
    }
    #[test]
    fn extraction() {
        let result =
            extract_text(include_bytes!("../../samples/01_petition.pdf").to_vec()).unwrap();
        assert_eq!(result["pages"].as_array().unwrap().len(), 3);
        assert!(result["pages"][0]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("petition"));
        assert!(extract_text(b"not PDF".to_vec()).is_err());
    }
}
