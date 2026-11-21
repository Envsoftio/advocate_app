//! Delete page-tree entries while retaining the document's other object graphs.
use super::{copy_catalog, finish_catalog, Dictionary, Object, ObjectRef, Result, Source, Writer};
use super::{COMPLEX, INVALID, MAX_DEPTH, MAX_OBJECTS};
use std::collections::{BTreeSet, HashSet};

pub(super) fn write(bytes: Vec<u8>, removed: &BTreeSet<usize>) -> Result<Vec<u8>> {
    let source = Source::open(bytes)?;
    if removed.is_empty()
        || removed.len() >= source.pages.len()
        || removed
            .last()
            .is_some_and(|&index| index >= source.pages.len())
    {
        return Err(INVALID);
    }
    let mut writer = copy_catalog(&source, None)?;
    writer.omit_null_page_attributes(&source)?;
    let catalog = source.catalog.as_dict().ok_or(INVALID)?;
    let root = catalog
        .get("Pages")
        .and_then(Object::as_reference)
        .ok_or(INVALID)?;
    prune_pages(&source, &mut writer, root, removed, 0)?;
    remove_links(&source, &mut writer, removed)?;
    if let Some(outline) = catalog.get("Outlines") {
        detach_bookmarks(&source, &mut writer, outline, removed)?;
    }
    if let Some(labels) = writer
        .reference_dict(Some(&writer.objects[0]))?
        .get("PageLabels")
        .cloned()
    {
        let ordered_removed = removed.iter().copied().collect::<Vec<_>>();
        let labels = shift_labels(&writer, &labels, &ordered_removed, &mut HashSet::new(), 0)?;
        let mut catalog = writer.reference_dict(Some(&writer.objects[0]))?;
        catalog.insert("PageLabels".into(), labels);
        writer.set(1, Object::Dictionary(catalog));
    }
    finish_catalog(writer, &source)
}

fn fields_at(writer: &Writer<'_>, source_id: ObjectRef) -> Result<(u32, Dictionary)> {
    let id = writer
        .imported
        .get(&(0, source_id))
        .copied()
        .ok_or(INVALID)?;
    let fields = writer.objects[id as usize - 1]
        .as_dict()
        .cloned()
        .ok_or(INVALID)?;
    Ok((id, fields))
}

fn prune_pages(
    source: &Source,
    writer: &mut Writer<'_>,
    branch: ObjectRef,
    removed: &BTreeSet<usize>,
    depth: usize,
) -> Result<usize> {
    if depth > MAX_DEPTH {
        return Err(COMPLEX);
    }
    let (id, mut fields) = fields_at(writer, branch)?;
    let original = source.pdf.load_object(branch).map_err(|_| INVALID)?;
    let children = original
        .as_dict()
        .and_then(|fields| fields.get("Kids"))
        .ok_or(INVALID)?;
    let children = source.resolve(children)?;
    let mut count = 0;
    let mut kept = Vec::new();
    for child in children.as_array().ok_or(INVALID)? {
        let child = child.as_reference().ok_or(INVALID)?;
        let pages = if let Some(index) = source.indexes.get(&child) {
            usize::from(!removed.contains(index))
        } else {
            prune_pages(source, writer, child, removed, depth + 1)?
        };
        if pages > 0 {
            count += pages;
            let copied = writer.imported.get(&(0, child)).copied().ok_or(INVALID)?;
            kept.push(super::reference(copied));
        }
    }
    fields.insert("Count".into(), Object::Integer(count as i64));
    fields.insert("Kids".into(), Object::Array(kept));
    writer.set(id, Object::Dictionary(fields));
    Ok(count)
}

fn goto_target(source: &Source, fields: &Dictionary) -> Result<Option<usize>> {
    let action = fields
        .get("A")
        .map(|value| source.resolve(value))
        .transpose()?;
    let destination = if let Some(action) = action
        .as_ref()
        .filter(|object| !matches!(object, Object::Null))
    {
        action
            .as_dict()
            .filter(|fields| fields.get("S").and_then(Object::as_name) == Some("GoTo"))
            .and_then(|fields| fields.get("D"))
    } else {
        fields.get("Dest")
    };
    match destination {
        Some(destination) => Ok(source.destination(destination, 0)?.map(|(page, _)| page)),
        None => Ok(None),
    }
}

fn remove_links(source: &Source, writer: &mut Writer<'_>, removed: &BTreeSet<usize>) -> Result<()> {
    for (index, &page) in source.pages.iter().enumerate() {
        if removed.contains(&index) {
            continue;
        }
        let original = source.pdf.load_object(page).map_err(|_| INVALID)?;
        let Some(list) = original.as_dict().and_then(|fields| fields.get("Annots")) else {
            continue;
        };
        let list = source.resolve(list)?;
        let Some(annotations) = list.as_array() else {
            continue;
        };
        let mut kept = Vec::new();
        for annotation in annotations {
            let object = source.resolve(annotation)?;
            let fields = object.as_dict().ok_or(INVALID)?;
            if fields.get("Subtype").and_then(Object::as_name) == Some("Link")
                && goto_target(source, fields)?.is_some_and(|page| removed.contains(&page))
            {
                continue;
            }
            kept.push(annotation.clone());
        }
        if kept.len() != annotations.len() {
            let (id, mut fields) = fields_at(writer, page)?;
            let kept = writer.copy(source, 0, Object::Array(kept), 0)?;
            fields.insert("Annots".into(), kept);
            writer.set(id, Object::Dictionary(fields));
        }
    }
    Ok(())
}

fn detach_bookmarks(
    source: &Source,
    writer: &mut Writer<'_>,
    outline: &Object,
    removed: &BTreeSet<usize>,
) -> Result<()> {
    let root = source.resolve(outline)?;
    let first = root
        .as_dict()
        .and_then(|fields| fields.get("First"))
        .and_then(Object::as_reference);
    let mut pending = first.into_iter().collect::<Vec<_>>();
    let mut visited = HashSet::new();
    while let Some(node) = pending.pop() {
        if visited.len() >= MAX_OBJECTS || !visited.insert(node) {
            return Err(COMPLEX);
        }
        let object = source.pdf.load_object(node).map_err(|_| INVALID)?;
        let fields = object.as_dict().ok_or(INVALID)?;
        pending.extend(
            ["Next", "First"]
                .into_iter()
                .filter_map(|key| fields.get(key)?.as_reference()),
        );
        if goto_target(source, fields)?.is_some_and(|page| removed.contains(&page)) {
            let (id, mut fields) = fields_at(writer, node)?;
            fields.remove("A");
            fields.remove("Dest");
            fields.insert("C".into(), Object::Array(vec![Object::Real(0.8); 3]));
            writer.set(id, Object::Dictionary(fields));
        }
    }
    Ok(())
}

fn shift_labels(
    writer: &Writer<'_>,
    node: &Object,
    removed: &[usize],
    visited: &mut HashSet<u32>,
    depth: usize,
) -> Result<Object> {
    if depth > MAX_DEPTH {
        return Err(COMPLEX);
    }
    if let Some(reference) = node.as_reference() {
        if !visited.insert(reference.id) {
            return Err(INVALID);
        }
    }
    let mut fields = writer.reference_dict(Some(node))?;
    // A rule at the deleted page remains at that position. Only earlier
    // deletions shift a rule. Keep rule order when indexes coincide.
    let shift = |value: &mut Object| {
        if let Object::Integer(index) = value {
            if let Ok(start) = usize::try_from(*index) {
                *index -= removed.partition_point(|&index| index < start) as i64;
            }
        }
    };
    for key in ["Nums", "Limits", "Kids"] {
        let Some(binding) = fields.get(key) else {
            continue;
        };
        let items = match binding.as_reference() {
            Some(reference) => writer
                .objects
                .get(reference.id as usize - 1)
                .ok_or(INVALID)?,
            None => binding,
        };
        let Object::Array(items) = items else {
            continue;
        };
        let mut items = items.clone();
        if key == "Kids" {
            for child in &mut items {
                *child = shift_labels(writer, child, removed, visited, depth + 1)?;
            }
        } else {
            let stride = if key == "Nums" { 2 } else { 1 };
            items.iter_mut().step_by(stride).for_each(shift);
        }
        fields.insert(key.into(), Object::Array(items));
    }
    Ok(Object::Dictionary(fields))
}
