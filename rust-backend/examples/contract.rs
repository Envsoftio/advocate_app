//! Disposable-store JSON-lines test driver; not an application service.
use std::io::{self, BufRead};
fn main() {
    let root = std::env::args().nth(1).expect("synthetic root");
    for line in io::stdin().lock().lines() {
        let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        // Reopen every operation to exercise persistence without a resident cache.
        let method = v["method"].as_str().unwrap();
        let path = v["path"].as_str().unwrap();
        let reply = if path.starts_with("/matters")
            || path.starts_with("/documents/")
            || path == "/output-presets"
            || path.split('?').next() == Some("/reader-position")
        {
            let mut store =
                matterdraft_backend::matters::Store::open(std::path::Path::new(&root)).unwrap();
            if method == "IMPORT" {
                let matter_id = path.split('/').nth(2).unwrap();
                let filename = v["body"]["filename"].as_str().unwrap();
                let bytes: Vec<u8> = v["body"]["bytes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_u64().unwrap() as u8)
                    .collect();
                store.import_pdf(matter_id, filename, bytes)
            } else if method == "SNAPSHOT" {
                let source = serde_json::from_value(v["body"].clone()).unwrap();
                let id = path.split('/').nth(2).unwrap();
                match store.open_pdf(id, &source) {
                    Ok(mut pdf) => {
                        let mut bytes = Vec::new();
                        pdf.copy_to(&mut bytes).unwrap();
                        matterdraft_backend::Reply {
                            status: 200,
                            body: serde_json::json!({"length":pdf.bytes,"bytes":bytes}),
                        }
                    }
                    Err(refusal) => refusal,
                }
            } else {
                store.request(method, path, v["body"].clone())
            }
        } else {
            matterdraft_backend::request(method, path)
        };
        println!("{}", serde_json::to_string(&reply).unwrap());
    }
}
