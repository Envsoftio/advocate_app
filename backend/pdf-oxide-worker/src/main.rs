//! Read-only PDF text worker. PDF bytes arrive over stdin, never a plaintext file.
use std::io::{self, Read, Write};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let streaming = args.as_slice() == ["--stream"];
    let selected = match args.as_slice() {
        [] => None,
        [flag] if flag == "--stream" => None,
        [flag, page] if flag == "--page" => Some(page.parse::<usize>()?),
        _ => return Err("unsupported operation".into()),
    };
    let mut bytes = Vec::new();
    io::stdin()
        .take(256 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 * 1024 {
        return Err("input limit".into());
    }
    let pdf = pdf_oxide::PdfDocument::from_bytes(bytes)?;
    if pdf.is_encrypted() {
        return Err("encrypted input unsupported".into());
    }
    let count = pdf.page_count()?;
    if streaming {
        let mut out = io::BufWriter::new(io::stdout().lock());
        serde_json::to_writer(
            &mut out,
            &serde_json::json!({
                "protocol": 2, "engine": "pdf_oxide", "version": "0.3.78",
                "kind": "header", "page_count": count
            }),
        )?;
        writeln!(out)?;
        out.flush()?;
        for page in 0..count {
            let text = pdf.extract_text(page)?;
            if text.len() > 10_000_000 {
                return Err("page text limit".into());
            }
            serde_json::to_writer(
                &mut out,
                &serde_json::json!({
                    "kind": "page", "page": page, "text": text
                }),
            )?;
            writeln!(out)?;
            out.flush()?;
        }
        serde_json::to_writer(
            &mut out,
            &serde_json::json!({"kind": "end", "page_count": count}),
        )?;
        writeln!(out)?;
        out.flush()?;
        return Ok(());
    }
    if selected.is_none() && count > 2000 {
        return Err("page limit".into());
    }
    let mut pages = Vec::with_capacity(if selected.is_some() { 1 } else { count });
    let mut characters = 0;
    if selected.is_some_and(|page| page >= count) {
        return Err("page outside document".into());
    }
    let start = selected.unwrap_or(0);
    let end = selected.map_or(count, |page| page + 1);
    for page in start..end {
        let text = pdf.extract_text(page)?;
        characters += text.chars().count();
        if characters > 10_000_000 {
            return Err("text limit".into());
        }
        pages.push(text);
    }
    serde_json::to_writer(
        io::stdout(),
        &serde_json::json!({
            "protocol": 1, "engine": "pdf_oxide", "version": "0.3.78", "pages": pages, "page_count": count, "start_page": start
        }),
    )?;
    Ok(())
}

fn main() {
    if run().is_err() {
        // Parser diagnostics may contain confidential PDF content. Do not log them.
        eprintln!("PDF Oxide could not extract this document within supported limits");
        std::process::exit(1);
    }
}
