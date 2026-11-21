use matterdraft_backend::planner::{plan_bundle, Bookmark, Document, Settings};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, io};

#[derive(Deserialize)]
struct Case {
    documents: Vec<Document>,
    document_titles: HashMap<String, String>,
    bookmarks: Vec<Bookmark>,
    max_pages: usize,
    keep_groups: bool,
    settings: Settings,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cases: Vec<Case> = serde_json::from_reader(io::stdin().lock())?;
    let results: Vec<_> = cases
        .iter()
        .map(|case| {
            match plan_bundle(
                &case.documents,
                &case.document_titles,
                &case.bookmarks,
                case.max_pages,
                case.keep_groups,
                &case.settings,
            ) {
                Ok(plan) => json!({"plan":plan}),
                Err(message) => json!({"error":message}),
            }
        })
        .collect();
    serde_json::to_writer(io::stdout().lock(), &results)?;
    Ok(())
}
