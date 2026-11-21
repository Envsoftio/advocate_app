use matterdraft_backend::admission::prepare_pdf;
use serde::Deserialize;
use serde_json::json;
use std::{fs, io, path::PathBuf};

#[derive(Deserialize)]
struct Case {
    source: PathBuf,
    working: PathBuf,
    password: Option<String>,
    acknowledge: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cases: Vec<Case> = serde_json::from_reader(io::stdin().lock())?;
    let mut responses = Vec::new();
    for case in cases {
        let source = fs::read(case.source)?;
        let response = match prepare_pdf(&source, case.password.as_deref(), case.acknowledge) {
            Ok(result) => {
                fs::write(case.working, &result.working_pdf)?;
                json!({"status":201,"page_count":result.page_count,
                    "ocr_status":if result.needs_ocr { "needs_ocr" } else { "not_requested" },
                    "signature":result.signature})
            }
            Err(error) => serde_json::to_value(error)?,
        };
        responses.push(response);
    }
    serde_json::to_writer(io::stdout().lock(), &responses)?;
    Ok(())
}
