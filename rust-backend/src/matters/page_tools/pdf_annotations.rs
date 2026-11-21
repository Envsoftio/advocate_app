use pdf_oxide::{object::Object, PdfDocument, Result};

pub(in crate::matters) fn field(
    pdf: &PdfDocument,
    parent: &Object,
    path: &[&str],
) -> Result<Option<Object>> {
    let mut object = parent.clone();
    for key in path {
        let Some(value) = object.as_dict().and_then(|fields| fields.get(*key)) else {
            return Ok(None);
        };
        object = pdf.resolve_object(value)?;
    }
    Ok((!matches!(object, Object::Null)).then_some(object))
}

pub(in crate::matters) fn annotations(pdf: &PdfDocument, page: usize) -> Result<Vec<Object>> {
    let page = pdf.get_page_for_debug(page)?;
    let Some(list) = field(pdf, &page, &["Annots"])? else {
        return Ok(Vec::new());
    };
    let Some(entries) = list.as_array() else {
        return Ok(Vec::new());
    };
    entries
        .iter()
        .map(|annotation| pdf.resolve_object(annotation))
        .collect()
}

fn link_destination(destination: Object) -> bool {
    matches!(destination, Object::Name(_) | Object::String(_))
        || destination
            .as_array()
            .is_some_and(|array| !array.is_empty())
}

fn has_file_name(pdf: &PdfDocument, file: &Object) -> Result<bool> {
    if matches!(file, Object::String(_)) {
        return Ok(true);
    }
    for key in ["UF", "F", "Unix", "DOS", "Mac"] {
        if let Some(name) = field(pdf, file, &[key])? {
            return Ok(matches!(name, Object::String(_)));
        }
    }
    Ok(false)
}

pub(in crate::matters) fn has_link(pdf: &PdfDocument, entry: &Object) -> Result<bool> {
    if field(pdf, entry, &["Subtype"])?
        .as_ref()
        .and_then(Object::as_name)
        != Some("Link")
        || field(pdf, entry, &["Rect"])?.is_none()
    {
        return Ok(false);
    }
    if let Some(destination) = field(pdf, entry, &["Dest"])? {
        return Ok(link_destination(destination));
    }
    let action = match field(pdf, entry, &["A"])? {
        Some(action) => Some(action),
        None => match field(pdf, entry, &["AA", "U"])? {
            Some(action) => Some(action),
            None => field(pdf, entry, &["AA", "D"])?,
        },
    };
    let Some(action) = action else {
        return Ok(false);
    };
    let kind = field(pdf, &action, &["S"])?;
    match kind.as_ref().and_then(Object::as_name) {
        Some("GoTo") => Ok(field(pdf, &action, &["D"])?.is_some_and(link_destination)),
        Some("URI") => Ok(true),
        Some("Launch" | "GoToR") => match field(pdf, &action, &["F"])? {
            Some(file) => has_file_name(pdf, &file),
            None => Ok(false),
        },
        Some("Named") => Ok(matches!(
            field(pdf, &action, &["N"])?
                .as_ref()
                .and_then(Object::as_name),
            Some("FirstPage" | "LastPage" | "PrevPage" | "NextPage")
        )),
        _ => Ok(false),
    }
}

pub(in crate::matters) fn link_rectangles(pdf: &PdfDocument, page: usize) -> Result<Vec<[f64; 4]>> {
    let mut links = Vec::new();
    for entry in annotations(pdf, page)? {
        if !has_link(pdf, &entry)? {
            continue;
        }
        let Some(rect) = field(pdf, &entry, &["Rect"])? else {
            continue;
        };
        let Some([a, b, c, d]) = rect.as_array().map(Vec::as_slice) else {
            continue;
        };
        let number = |value: &Object| {
            value
                .as_real()
                .or_else(|| value.as_integer().map(|integer| integer as f64))
        };
        if let [Some(x0), Some(y0), Some(x1), Some(y1)] =
            [number(a), number(b), number(c), number(d)]
        {
            links.push([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]);
        }
    }
    Ok(links)
}
