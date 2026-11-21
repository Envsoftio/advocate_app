//! Synthetic-only capability probe; deliberately not used by production routes.
use pdf_oxide::editor::DocumentEditor;
use std::{env, fs};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = env::args().collect();
    let mut doc = DocumentEditor::open(&a[2])?;
    match a[1].as_str() {
        "rotate" => doc.set_page_rotation(0, 90)?,
        "reverse" => doc.select_pages(&[2, 1, 0])?,
        "merge" => {
            doc.merge_from(&a[2])?;
        }
        "redact" => {
            doc.add_redaction(0, [0., 0., 595., 842.], Some([0., 0., 0.]))?;
            let report = doc.apply_redactions_destructive(Default::default())?;
            eprintln!("{report:?}");
        }
        _ => return Err("unknown operation".into()),
    }
    fs::write(&a[3], doc.save_to_bytes()?)?;
    Ok(())
}
