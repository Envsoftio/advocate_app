//! Copy field trees for fresh page assemblies. Each selected occurrence owns
//! its own form fields and points to the same widget objects as the page.
use super::{pdf_text, reference, Dictionary, Object, ObjectRef, Page, Result, Source, Writer};
use super::{COMPLEX, INVALID, MAX_DEPTH, MAX_OBJECTS};
use pdf_oxide::optional_content::decode_pdf_text_string;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Forms {
    defaults: Option<Dictionary>,
    fields: Vec<Object>,
    calculation_order: Vec<Object>,
    names: HashSet<String>,
}

struct Field {
    id: u32,
    parent: Option<u32>,
    children: Vec<u32>,
    original: Dictionary,
}

impl Forms {
    pub(super) fn page(
        &mut self,
        writer: &mut Writer<'_>,
        source: &Source,
        page: &Page,
        widgets: Vec<Object>,
    ) -> Result<Vec<Object>> {
        let Some(form) = source
            .form
            .get_or_init(|| read_form(source))
            .as_ref()
            .map_err(|error| *error)?
        else {
            return Ok(Vec::new());
        };
        if self.fields.is_empty() {
            let mut defaults = form.clone();
            defaults.remove("Fields");
            defaults.remove("CO");
            self.defaults = Some(writer.copy_dict(source, page.source, defaults, 0)?);
        }
        let mut nodes = HashMap::new();
        // The form's top-level parent fields precede standalone widgets on
        // each copied page. Page annotation order still follows the source.
        for original in widgets.iter().filter_map(Object::as_reference) {
            if nodes.contains_key(&original) {
                continue;
            }
            let root = root_of(source, original)?;
            if root != original {
                self.copy_root(source, writer, &mut nodes, root)?;
                if !nodes.contains_key(&original) {
                    return Err(INVALID);
                }
            }
        }
        let mut references = Vec::new();
        for widget in widgets {
            let Some(original) = widget.as_reference() else {
                let mut fields = widget.as_dict().cloned().ok_or(INVALID)?;
                fields.remove("P");
                fields.remove("Parent");
                fields.remove("Kids");
                let id = writer.reserve()?;
                self.unique_name(source, id, &mut fields)?;
                self.fields.push(reference(id));
                if calculation(source, &fields)? {
                    self.calculation_order.push(reference(id));
                }
                let fields = writer.copy_dict(source, page.source, fields, 0)?;
                writer.set(id, Object::Dictionary(fields));
                references.push(reference(id));
                continue;
            };
            if !nodes.contains_key(&original) {
                self.copy_root(source, writer, &mut nodes, original)?;
            }
            let node = &nodes[&original];
            references.push(reference(node.id));
            if calculation(source, &node.original)? {
                self.calculation_order.push(reference(node.id));
            }
        }
        let bindings = nodes
            .iter()
            .map(|(&object, node)| {
                let key = (page.source, object);
                (key, writer.imported.insert(key, node.id))
            })
            .collect::<Vec<_>>();
        let result = write_fields(writer, source, page, nodes);
        for (key, old) in bindings {
            match old {
                Some(id) => {
                    writer.imported.insert(key, id);
                }
                None => {
                    writer.imported.remove(&key);
                }
            }
        }
        result?;
        Ok(references)
    }

    fn copy_root(
        &mut self,
        source: &Source,
        writer: &mut Writer<'_>,
        nodes: &mut HashMap<ObjectRef, Field>,
        root: ObjectRef,
    ) -> Result<()> {
        let id = collect(source, writer, nodes, root, None, 0)?;
        let field = nodes.get_mut(&root).ok_or(INVALID)?;
        self.unique_name(source, id, &mut field.original)?;
        self.fields.push(reference(id));
        Ok(())
    }

    fn unique_name(&mut self, source: &Source, id: u32, fields: &mut Dictionary) -> Result<()> {
        let title = fields
            .get("T")
            .map(|value| source.resolve(value))
            .transpose()?
            .and_then(|value| value.as_string().map(decode_pdf_text_string))
            .unwrap_or_default();
        if self.names.insert(title.clone()) {
            return Ok(());
        }
        let mut candidate = format!("{title} [{id}]");
        while !self.names.insert(candidate.clone()) {
            candidate.push_str(&format!(" [{id}]"));
        }
        fields.insert("T".into(), pdf_text(&candidate));
        Ok(())
    }

    pub(super) fn install(self, writer: &mut Writer<'_>, catalog: &mut Dictionary) -> Result<()> {
        if let Some(mut form) = self.defaults {
            form.insert("Fields".into(), Object::Array(self.fields));
            form.insert("CO".into(), Object::Array(self.calculation_order));
            catalog.insert(
                "AcroForm".into(),
                reference(writer.add(Object::Dictionary(form))?),
            );
        }
        Ok(())
    }
}

fn read_form(source: &Source) -> Result<Option<Dictionary>> {
    let Some(form) = source
        .catalog
        .as_dict()
        .and_then(|catalog| catalog.get("AcroForm"))
    else {
        return Ok(None);
    };
    let form = source.resolve(form)?;
    let Some(fields) = form.as_dict().and_then(|fields| fields.get("Fields")) else {
        return Ok(None);
    };
    let fields = source.resolve(fields)?;
    Ok(fields
        .as_array()
        .filter(|fields| !fields.is_empty())
        .and_then(|_| form.as_dict().cloned()))
}

fn root_of(source: &Source, mut original: ObjectRef) -> Result<ObjectRef> {
    let mut seen = HashSet::new();
    while seen.len() <= MAX_DEPTH && seen.insert(original) {
        let value = source.pdf.load_object(original).map_err(|_| INVALID)?;
        let fields = value.as_dict().ok_or(INVALID)?;
        match fields.get("Parent").and_then(Object::as_reference) {
            Some(parent) => original = parent,
            None => return Ok(original),
        }
    }
    Err(COMPLEX)
}

fn collect(
    source: &Source,
    writer: &mut Writer<'_>,
    nodes: &mut HashMap<ObjectRef, Field>,
    original: ObjectRef,
    parent: Option<u32>,
    depth: usize,
) -> Result<u32> {
    if depth > MAX_DEPTH || nodes.len() >= MAX_OBJECTS || nodes.contains_key(&original) {
        return Err(COMPLEX);
    }
    let object = source.pdf.load_object(original).map_err(|_| INVALID)?;
    let mut fields = object.as_dict().cloned().ok_or(INVALID)?;
    let children = fields
        .remove("Kids")
        .map(|value| source.resolve(&value))
        .transpose()?;
    fields.remove("Parent");
    // Selected widgets find their page through Annots. An off-page sibling
    // of a shared field can carry a value and appearance, without a page.
    fields.remove("P");
    let id = writer.reserve()?;
    nodes.insert(
        original,
        Field {
            id,
            parent,
            children: Vec::new(),
            original: fields,
        },
    );
    let mut kids = Vec::new();
    if let Some(children) = children.as_ref().and_then(Object::as_array) {
        for child in children {
            let child = child.as_reference().ok_or(INVALID)?;
            kids.push(collect(source, writer, nodes, child, Some(id), depth + 1)?);
        }
    }
    nodes.get_mut(&original).ok_or(INVALID)?.children = kids;
    Ok(id)
}

fn write_fields(
    writer: &mut Writer<'_>,
    source: &Source,
    page: &Page,
    nodes: HashMap<ObjectRef, Field>,
) -> Result<()> {
    let mut nodes = nodes.into_values().collect::<Vec<_>>();
    nodes.sort_unstable_by_key(|field| field.id);
    for field in nodes {
        let mut fields = writer.copy_dict(source, page.source, field.original, 0)?;
        if let Some(parent) = field.parent {
            fields.insert("Parent".into(), reference(parent));
        }
        if !field.children.is_empty() {
            fields.insert(
                "Kids".into(),
                Object::Array(field.children.into_iter().map(reference).collect()),
            );
        }
        writer.set(field.id, Object::Dictionary(fields));
    }
    Ok(())
}

fn calculation(source: &Source, fields: &Dictionary) -> Result<bool> {
    let Some(actions) = fields.get("AA") else {
        return Ok(false);
    };
    let actions = source.resolve(actions)?;
    let Some(calculation) = actions.as_dict().and_then(|fields| fields.get("C")) else {
        return Ok(false);
    };
    Ok(source.resolve(calculation)?.as_dict().is_some())
}
