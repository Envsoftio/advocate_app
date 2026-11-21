//! Keep the structure tree's content references aligned with edited Form placements.
use super::{
    dict, name, pdf_text, reference, Dictionary, ReplacementTag, Result, RetainedAnchor,
    SharedActualTextEdit, Writer, MAX_DEPTH,
};
use crate::matters::{redaction_vectors::marked_commands, text_edit::reading_order::CONTINUATION};
use pdf_oxide::{
    content::{parse_content_stream, Operator, TextElement},
    object::Object,
    optional_content::decode_pdf_text_string,
    writer::ObjectSerializer,
};
use std::collections::{BTreeMap, HashSet};

const INVALID: &str = "Cannot preserve the tagged Form's structure during text editing";
const MAX_TAG_ITEMS: usize = 100_000;
const MAX_CONTENT_BYTES: usize = 64 * 1024 * 1024;

fn id(value: Option<&Object>) -> Option<u32> {
    value
        .and_then(Object::as_reference)
        .map(|reference| reference.id)
}

struct ParentTree {
    root: Object,
    fields: Dictionary,
    entries: BTreeMap<i64, Object>,
}

impl ParentTree {
    fn read(writer: &Writer<'_>) -> Result<Option<Self>> {
        let catalog = writer.object(1)?.as_dict().ok_or(INVALID)?;
        let Some(root) = catalog.get("StructTreeRoot").cloned() else {
            return Ok(None);
        };
        let fields = writer
            .resolve_written(&root)?
            .as_dict()
            .cloned()
            .ok_or(INVALID)?;
        let mut entries = BTreeMap::new();
        let mut seen = HashSet::new();
        let mut pending = fields.get("ParentTree").into_iter().collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            if let Some(key) = id(Some(node)) {
                if !seen.insert(key) || seen.len() > MAX_TAG_ITEMS {
                    return Err(INVALID);
                }
            }
            let node = writer.resolve_written(node)?.as_dict().ok_or(INVALID)?;
            if let Some(nums) = node.get("Nums") {
                let pairs = writer.resolve_written(nums)?.as_array().ok_or(INVALID)?;
                if pairs.len() % 2 != 0 {
                    return Err(INVALID);
                }
                for pair in pairs.chunks_exact(2) {
                    let key = pair[0]
                        .as_integer()
                        .filter(|&key| key >= 0)
                        .ok_or(INVALID)?;
                    if entries.insert(key, pair[1].clone()).is_some()
                        || entries.len() > MAX_TAG_ITEMS
                    {
                        return Err(INVALID);
                    }
                }
            }
            if let Some(kids) = node.get("Kids") {
                let kids = writer.resolve_written(kids)?.as_array().ok_or(INVALID)?;
                if kids.len() + pending.len() > MAX_TAG_ITEMS {
                    return Err(INVALID);
                }
                pending.extend(kids);
            }
        }
        Ok(Some(Self {
            root,
            fields,
            entries,
        }))
    }

    fn add(&mut self, value: Object) -> Result<i64> {
        let minimum = self
            .entries
            .last_key_value()
            .map_or(0, |(&key, _)| key.saturating_add(1));
        let key = self
            .fields
            .get("ParentTreeNextKey")
            .and_then(Object::as_integer)
            .unwrap_or(minimum)
            .max(minimum);
        let next = key.checked_add(1).ok_or(INVALID)?;
        self.entries.insert(key, value);
        self.fields
            .insert("ParentTreeNextKey".into(), Object::Integer(next));
        Ok(key)
    }

    fn write(mut self, writer: &mut Writer<'_>) -> Result<()> {
        let nums = self
            .entries
            .into_iter()
            .flat_map(|(key, value)| [Object::Integer(key), value])
            .collect();
        let tree = Object::Dictionary(dict([("Nums", Object::Array(nums))]));
        if let Some(key) = id(self.fields.get("ParentTree")) {
            writer.set(key, tree);
        } else {
            self.fields.insert("ParentTree".into(), tree);
        }
        let root = Object::Dictionary(self.fields);
        if let Some(key) = id(Some(&self.root)) {
            writer.set(key, root);
        } else {
            let mut catalog = writer.object(1)?.as_dict().cloned().ok_or(INVALID)?;
            catalog.insert("StructTreeRoot".into(), root);
            writer.set(1, Object::Dictionary(catalog));
        }
        Ok(())
    }
}

impl Writer<'_> {
    fn object(&self, key: u32) -> Result<&Object> {
        self.objects
            .get(key.checked_sub(1).ok_or(INVALID)? as usize)
            .ok_or(INVALID)
    }

    fn resolve_written<'a>(&'a self, value: &'a Object) -> Result<&'a Object> {
        let mut value = value;
        for _ in 0..MAX_DEPTH {
            match id(Some(value)) {
                Some(key) => value = self.object(key)?,
                None => return Ok(value),
            }
        }
        Err(INVALID)
    }

    fn page_operators(&self, page: &Dictionary) -> Result<Vec<Operator>> {
        let mut content = Vec::new();
        for stream in self.contents(&mut page.clone())? {
            content.extend(
                self.resolve_written(&stream)?
                    .decode_stream_data()
                    .map_err(|_| INVALID)?,
            );
            content.push(b'\n');
            if content.len() > MAX_CONTENT_BYTES {
                return Err(INVALID);
            }
        }
        parse_content_stream(&content).map_err(|_| INVALID)
    }

    fn painted_forms(&self, page: u32, targets: &HashSet<u32>) -> Result<HashSet<u32>> {
        let fields = self.object(page)?.as_dict().ok_or(INVALID)?;
        let mut active = HashSet::new();
        let mut visited = HashSet::new();
        let mut decoded = 0;
        let mut pending = vec![(self.page_operators(fields)?, fields.get("Resources"), page)];
        while let Some((ops, inherited, owner)) = pending.pop() {
            let resources = inherited
                .map(|value| self.resolve_written(value))
                .transpose()?
                .and_then(Object::as_dict);
            let forms = resources
                .and_then(|fields| fields.get("XObject"))
                .map(|value| self.resolve_written(value))
                .transpose()?
                .and_then(Object::as_dict);
            let Some(forms) = forms else { continue };
            for op in ops {
                let Operator::Do { name } = op else { continue };
                let key = id(forms.get(&name)).ok_or(INVALID)?;
                if targets.contains(&key) {
                    active.insert(key);
                    if active == *targets {
                        return Ok(active);
                    }
                }
                let object = self.object(key)?;
                let fields = object.as_dict().ok_or(INVALID)?;
                if fields.get("Subtype").and_then(Object::as_name) != Some("Form") {
                    continue;
                }
                let next = match fields.get("Resources") {
                    Some(value) if !matches!(self.resolve_written(value)?, Object::Null) => {
                        Some(value)
                    }
                    _ => None,
                };
                let next_owner = if next.is_some() { key } else { owner };
                if !visited.insert((key, next_owner)) {
                    continue;
                }
                if visited.len() > MAX_TAG_ITEMS {
                    return Err(INVALID);
                }
                let data = object.decode_stream_data().map_err(|_| INVALID)?;
                decoded += data.len();
                if decoded > MAX_CONTENT_BYTES {
                    return Err(INVALID);
                }
                pending.push((
                    parse_content_stream(&data).map_err(|_| INVALID)?,
                    next.or(inherited),
                    next_owner,
                ));
            }
        }
        Ok(active)
    }

    fn set_form_key(&mut self, form: u32, field: &str, key: Option<i64>) -> Result<()> {
        let Object::Stream { dict, data } = self.object(form)?.clone() else {
            return Err(INVALID);
        };
        let mut dict = dict;
        if let Some(key) = key {
            dict.insert(field.into(), Object::Integer(key));
        } else {
            dict.remove(field);
        }
        self.set(form, Object::Stream { dict, data });
        Ok(())
    }

    pub(super) fn retarget_rebound_form_tags(&mut self, page: u32) -> Result<()> {
        let mut first = HashSet::new();
        let additional = self
            .form_copies
            .iter()
            .filter_map(|&(original, copy)| (!first.insert(original)).then_some(copy))
            .collect::<Vec<_>>();
        // Multiple font contexts may require several copies for one original
        // stream. Its existing structure reference still owns one logical
        // rendition on this page.
        for copy in additional {
            self.set_form_key(copy, "StructParents", None)?;
            self.set_form_key(copy, "StructParent", None)?;
        }
        self.retarget_form_tags(page, None)?;
        Ok(())
    }

    pub(super) fn retarget_form_tags(
        &mut self,
        page: u32,
        selected: Option<ReplacementTag>,
    ) -> Result<Option<ReplacementTag>> {
        let mut copies = Vec::new();
        for (reference, copy) in std::mem::take(&mut self.form_copies) {
            let fields = self.object(copy)?.as_dict().ok_or(INVALID)?;
            if fields.contains_key("StructParents") || fields.contains_key("StructParent") {
                let original = *self.imported.get(&(0, reference)).ok_or(INVALID)?;
                copies.push((original, copy));
            }
        }
        if copies.is_empty() {
            return Ok(None);
        }
        let mut tree = ParentTree::read(self)?;
        let mut requested = None;
        let mut tree_changed = false;
        let originals = copies.iter().map(|&(original, _)| original).collect();
        let painted = self.painted_forms(page, &originals)?;
        let mut transferred = HashSet::new();
        for (original, copy) in copies {
            let fields = self.object(original)?.as_dict().ok_or(INVALID)?;
            let field = match (
                fields.contains_key("StructParents"),
                fields.contains_key("StructParent"),
            ) {
                (true, false) => "StructParents",
                (false, true) => "StructParent",
                _ => return Err(INVALID),
            };
            if tree.is_none() || painted.contains(&original) {
                self.set_form_key(copy, field, None)?;
                continue;
            }
            // An internal Form tag identifies its stream, whereas an OBJR may
            // identify one whole-Form rendition per page.
            let key = self
                .resolve_written(fields.get(field).ok_or(INVALID)?)?
                .as_integer()
                .ok_or(INVALID)?;
            if !transferred.insert(key) {
                return Err(INVALID);
            }
            let tree = tree.as_mut().ok_or(INVALID)?;
            let value = tree.entries.get(&key).ok_or(INVALID)?.clone();
            let (moved, kept, matches_selection) = if field == "StructParents" {
                let owners = self
                    .resolve_written(&value)?
                    .as_array()
                    .ok_or(INVALID)?
                    .clone();
                let mut moved = vec![Object::Null; owners.len()];
                let mut kept = owners;
                for (mcid, owner) in kept.iter_mut().enumerate() {
                    if matches!(owner, Object::Null) {
                        continue;
                    }
                    let (changed, remaining) =
                        self.retarget_owner(owner, Some(mcid), page, original, copy)?;
                    if changed {
                        moved[mcid] = owner.clone();
                    }
                    if !remaining {
                        *owner = Object::Null;
                    }
                }
                let has_items =
                    |items: &[Object]| items.iter().any(|item| !matches!(item, Object::Null));
                let matches_selection = selected
                    .filter(|tag| tag.parent_key == key)
                    .and_then(|tag| moved.get(tag.mcid?))
                    .is_some_and(|item| !matches!(item, Object::Null));
                (
                    has_items(&moved).then_some(Object::Array(moved)),
                    has_items(&kept).then_some(Object::Array(kept)),
                    matches_selection,
                )
            } else {
                let (changed, remaining) =
                    self.retarget_owner(&value, None, page, original, copy)?;
                (
                    changed.then(|| value.clone()),
                    remaining.then_some(value),
                    changed
                        && selected.is_some_and(|tag| tag.parent_key == key && tag.mcid.is_none()),
                )
            };
            let Some(moved) = moved else {
                self.set_form_key(copy, field, None)?;
                continue;
            };
            let target = if let Some(kept) = kept {
                tree.entries.insert(key, kept);
                tree_changed = true;
                tree.add(moved)?
            } else {
                self.set_form_key(original, field, None)?;
                key
            };
            self.set_form_key(copy, field, Some(target))?;
            if matches_selection {
                requested = selected.map(|tag| ReplacementTag {
                    parent_key: target,
                    ..tag
                });
            }
        }
        if tree_changed {
            tree.ok_or(INVALID)?.write(self)?;
        }
        Ok(requested)
    }

    fn owner_kids(&self, owner: &Dictionary) -> Result<Vec<Object>> {
        let kid = owner.get("K").ok_or(INVALID)?;
        Ok(self
            .resolve_written(kid)?
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![kid.clone()]))
    }

    fn has_page_mark(
        &self,
        owner: &Dictionary,
        kids: &[Object],
        page: u32,
        mcid: usize,
    ) -> Result<bool> {
        let mcid = i64::try_from(mcid).map_err(|_| INVALID)?;
        for kid in kids {
            match self.resolve_written(kid)? {
                Object::Integer(value) if *value == mcid && id(owner.get("Pg")) == Some(page) => {
                    return Ok(true);
                }
                Object::Dictionary(fields)
                    if fields.get("Type").and_then(Object::as_name) == Some("MCR")
                        && fields.get("MCID").and_then(Object::as_integer) == Some(mcid)
                        && fields
                            .get("Stm")
                            .is_none_or(|value| matches!(value, Object::Null))
                        && id(fields.get("Pg").or_else(|| owner.get("Pg"))) == Some(page) =>
                {
                    return Ok(true);
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn clear_actual_text(
        &mut self,
        owner_id: u32,
        owner: &mut Dictionary,
        kids: &[Object],
    ) -> Result<()> {
        if kids.len() != 1 {
            return Err(INVALID);
        }
        if owner.remove("ActualText").is_some() {
            return Ok(());
        }
        // An ancestor can provide the alternate string. Clear it only if
        // the selected anchor accounts for all of that ancestor's content.
        let mut child = owner_id;
        let mut parent = id(owner.get("P"));
        for _ in 0..MAX_DEPTH {
            let key = parent.ok_or(INVALID)?;
            let mut fields = self.object(key)?.as_dict().cloned().ok_or(INVALID)?;
            let kids = self.owner_kids(&fields)?;
            if kids.len() != 1 || id(kids.first()) != Some(child) {
                return Err(INVALID);
            }
            if fields.remove("ActualText").is_some() {
                self.set(key, Object::Dictionary(fields));
                return Ok(());
            }
            child = key;
            parent = id(fields.get("P"));
        }
        Err(INVALID)
    }

    fn retarget_owner(
        &mut self,
        owner: &Object,
        mcid: Option<usize>,
        page: u32,
        original: u32,
        copy: u32,
    ) -> Result<(bool, bool)> {
        let owner_id = id(Some(owner)).ok_or(INVALID)?;
        let mut owner = self.object(owner_id)?.as_dict().cloned().ok_or(INVALID)?;
        let mut kids = self.owner_kids(&owner)?;
        let (kind, link) = if mcid.is_some() {
            ("MCR", "Stm")
        } else {
            ("OBJR", "Obj")
        };
        let mut changed = false;
        let mut remaining = false;
        for kid in &mut kids {
            let Some(content) = self.resolve_written(kid)?.as_dict() else {
                continue;
            };
            if content.get("Type").and_then(Object::as_name) != Some(kind)
                || id(content.get(link)) != Some(original)
                || mcid.is_some_and(|mcid| {
                    content.get("MCID").and_then(Object::as_integer) != Some(mcid as i64)
                })
            {
                continue;
            }
            let member_page = id(content.get("Pg").or_else(|| owner.get("Pg"))).ok_or(INVALID)?;
            if member_page == page {
                let mut content = content.clone();
                content.insert(link.into(), reference(copy));
                *kid = Object::Dictionary(content);
                changed = true;
            } else {
                remaining = true;
            }
        }
        if !changed && !remaining {
            return Err(INVALID);
        }
        if changed {
            owner.insert("K".into(), Object::Array(kids));
            self.set(owner_id, Object::Dictionary(owner));
        }
        Ok((changed, remaining))
    }

    fn actual_text_anchor_count(&self, owner: &Dictionary) -> Result<usize> {
        let mut pending = self.owner_kids(owner)?;
        let mut walked = 0;
        let mut count = 0;
        while let Some(item) = pending.pop() {
            walked += 1;
            if walked > MAX_TAG_ITEMS {
                return Err(INVALID);
            }
            match self.resolve_written(&item)? {
                Object::Integer(_) => count += 1,
                Object::Dictionary(fields) => match fields.get("Type").and_then(Object::as_name) {
                    Some("MCR") => count += 1,
                    Some("StructElem") | None if fields.contains_key("S") => {
                        if !fields.contains_key("ActualText") {
                            let kids = self.owner_kids(fields)?;
                            if pending.len() + kids.len() > MAX_TAG_ITEMS {
                                return Err(INVALID);
                            }
                            pending.extend(kids);
                        }
                    }
                    _ => return Err(INVALID),
                },
                _ => return Err(INVALID),
            }
        }
        Ok(count)
    }

    fn update_shared_actual_text(
        &mut self,
        owner_id: u32,
        owner: &mut Dictionary,
        edit: &SharedActualTextEdit,
    ) -> Result<()> {
        let update = |writer: &Self, fields: &mut Dictionary| -> Result<bool> {
            let Some(original) = fields.get("ActualText") else {
                return Ok(false);
            };
            let original = writer
                .resolve_written(original)?
                .as_string()
                .ok_or(INVALID)?;
            if decode_pdf_text_string(original) != edit.original
                || writer.actual_text_anchor_count(fields)? != edit.anchors
            {
                return Err(INVALID);
            }
            fields.insert("ActualText".into(), pdf_text(&edit.replacement));
            Ok(true)
        };
        if update(self, owner)? {
            return Ok(());
        }
        let mut child = owner_id;
        let mut parent = id(owner.get("P"));
        for _ in 0..MAX_DEPTH {
            let key = parent.ok_or(INVALID)?;
            let mut fields = self.object(key)?.as_dict().cloned().ok_or(INVALID)?;
            if !self
                .owner_kids(&fields)?
                .iter()
                .any(|value| value.as_reference().is_some_and(|item| item.id == child))
            {
                return Err(INVALID);
            }
            if update(self, &mut fields)? {
                self.set(key, Object::Dictionary(fields));
                return Ok(());
            }
            child = key;
            parent = id(fields.get("P"));
        }
        Err(INVALID)
    }

    fn replacement_anchor(
        &self,
        owner: &Dictionary,
        kids: &[Object],
        page_id: u32,
        tag: &ReplacementTag,
    ) -> Result<usize> {
        let mcid = i64::try_from(tag.mcid.ok_or(INVALID)?).map_err(|_| INVALID)?;
        let mut target = None;
        for (index, kid) in kids.iter().enumerate() {
            let matches = match self.resolve_written(kid)? {
                Object::Integer(value) => {
                    tag.from_page && *value == mcid && id(owner.get("Pg")) == Some(page_id)
                }
                Object::Dictionary(fields)
                    if fields.get("Type").and_then(Object::as_name) == Some("MCR")
                        && fields.get("MCID").and_then(Object::as_integer) == Some(mcid)
                        && id(fields.get("Pg").or_else(|| owner.get("Pg"))) == Some(page_id) =>
                {
                    match fields.get("Stm") {
                        None | Some(Object::Null) => tag.from_page,
                        Some(stm) => {
                            let key = self
                                .resolve_written(stm)?
                                .as_dict()
                                .and_then(|stream| stream.get("StructParents"))
                                .map(|value| self.resolve_written(value))
                                .transpose()?
                                .and_then(Object::as_integer);
                            !tag.from_page && key == Some(tag.parent_key)
                        }
                    }
                }
                _ => false,
            };
            if matches && target.replace(index).is_some() {
                return Err(INVALID);
            }
        }
        target.ok_or(INVALID)
    }

    fn write_continuation(&mut self, streams: &[u32], mcid: usize) -> Result<()> {
        let mut changed = None;
        let marker = format!("/{CONTINUATION}");
        for &stream in streams {
            let object = self.object(stream)?;
            let data = object.decode_stream_data().map_err(|_| INVALID)?;
            if data.len() > MAX_CONTENT_BYTES {
                return Err(INVALID);
            }
            if !data
                .windows(CONTINUATION.len())
                .any(|word| word == CONTINUATION.as_bytes())
            {
                continue;
            }
            for range in marked_commands(&data).map_err(|_| INVALID)? {
                if !data[range.clone()].starts_with(marker.as_bytes()) {
                    continue;
                }
                let operators = parse_content_stream(&data[range.clone()]).map_err(|_| INVALID)?;
                let [Operator::BeginMarkedContentDict { tag, properties }] = operators.as_slice()
                else {
                    return Err(INVALID);
                };
                let mut props = properties.as_dict().cloned().ok_or(INVALID)?;
                if tag != CONTINUATION
                    || changed.is_some()
                    || props.keys().any(|key| key != "ActualText")
                    || props
                        .get("ActualText")
                        .is_some_and(|value| value.as_string().is_none())
                {
                    return Err(INVALID);
                }
                let mut dict = object.as_dict().cloned().ok_or(INVALID)?;
                for key in ["Length", "Filter", "DecodeParms"] {
                    dict.remove(key);
                }
                props.insert("MCID".into(), Object::Integer(mcid as i64));
                let mut revised = data[..range.start].to_vec();
                revised.extend_from_slice(b"/Span ");
                revised.extend(ObjectSerializer::compact().serialize(&Object::Dictionary(props)));
                revised.extend_from_slice(b" BDC");
                revised.extend_from_slice(&data[range.end..]);
                changed = Some((
                    stream,
                    Object::Stream {
                        dict,
                        data: revised.into(),
                    },
                ));
            }
        }
        let (id, stream) = changed.ok_or(INVALID)?;
        self.set(id, stream);
        Ok(())
    }

    fn empty_mark(&self, ops: &[Operator], props: &Dictionary, mcid: usize) -> Option<bool> {
        let mut depth = 0usize;
        let mut active = None;
        let mut seen = false;
        for operator in ops {
            match operator {
                Operator::BeginMarkedContentDict { properties, .. } => {
                    if active.is_some() {
                        return Some(false);
                    }
                    let properties = match properties.as_name() {
                        Some(name) => self.resolve_written(props.get(name)?).ok()?,
                        None => properties,
                    };
                    let fields = properties.as_dict()?;
                    if fields.get("MCID").and_then(Object::as_integer) == Some(mcid as i64) {
                        if fields.len() != 1 || seen {
                            return Some(false);
                        }
                        seen = true;
                        active = Some(depth);
                    }
                    depth += 1;
                }
                Operator::BeginMarkedContent { .. } => {
                    if active.is_some() {
                        return Some(false);
                    }
                    depth += 1;
                }
                Operator::EndMarkedContent => {
                    depth = depth.checked_sub(1)?;
                    if active == Some(depth) {
                        active = None;
                    }
                }
                _ if active.is_none() => {}
                Operator::Tj { text } if text.is_empty() => {}
                Operator::TJ { array }
                    if array.iter().all(|item| match item {
                        TextElement::Offset(_) => true,
                        TextElement::String(text) => text.is_empty(),
                    }) => {}
                Operator::SaveState
                | Operator::RestoreState
                | Operator::Cm { .. }
                | Operator::BeginText
                | Operator::EndText
                | Operator::Tm { .. }
                | Operator::Td { .. }
                | Operator::TD { .. }
                | Operator::TStar
                | Operator::Tc { .. }
                | Operator::Tw { .. }
                | Operator::Tz { .. }
                | Operator::TL { .. }
                | Operator::Tf { .. }
                | Operator::Tr { .. }
                | Operator::Ts { .. }
                | Operator::SetFillGray { .. }
                | Operator::SetFillRgb { .. }
                | Operator::SetFillCmyk { .. } => {}
                _ => return Some(false),
            }
        }
        Some(seen && depth == 0)
    }

    pub(super) fn tag_replacement(
        &mut self,
        page: &mut Dictionary,
        page_id: u32,
        tag: &ReplacementTag,
        shared_actual_text: Option<&SharedActualTextEdit>,
    ) -> Result<usize> {
        let mut tree = ParentTree::read(self)?.ok_or(INVALID)?;
        let parent = tree.entries.get(&tag.parent_key).ok_or(INVALID)?;
        let parent = if let Some(mcid) = tag.mcid {
            self.resolve_written(parent)?
                .as_array()
                .and_then(|items| items.get(mcid))
                .ok_or(INVALID)?
        } else {
            parent
        };
        let owner_id = id(Some(parent)).ok_or(INVALID)?;
        let mut owner = self.object(owner_id)?.as_dict().cloned().ok_or(INVALID)?;
        let mut kids = self.owner_kids(&owner)?;
        let page_key = page
            .get("StructParents")
            .map(|value| self.resolve_written(value)?.as_integer().ok_or(INVALID))
            .transpose()?;
        if tag.from_page
            && (page_key != Some(tag.parent_key)
                || !self.has_page_mark(&owner, &kids, page_id, tag.mcid.ok_or(INVALID)?)?)
        {
            return Err(INVALID);
        }
        let mut anchor = (shared_actual_text.is_some() || tag.retained_anchor.is_some())
            .then(|| self.replacement_anchor(&owner, &kids, page_id, tag))
            .transpose()?;
        if let Some(shared) = shared_actual_text {
            if tag.retained_anchor.is_some() {
                return Err(INVALID);
            }
            self.update_shared_actual_text(owner_id, &mut owner, shared)?;
        } else if tag.remove_actual_text {
            self.clear_actual_text(owner_id, &mut owner, &kids)?;
        }
        let mut items = match page_key {
            Some(key) => self
                .resolve_written(tree.entries.get(&key).ok_or(INVALID)?)?
                .as_array()
                .cloned()
                .ok_or(INVALID)?,
            None => Vec::new(),
        };
        let resources = self.reference_dict(page.get("Resources"))?;
        let props = self.reference_dict(resources.get("Properties"))?;
        let page_ops = self.page_operators(page)?;
        if anchor.is_none() && kids.len() > 1 {
            // Re-editing an emptied text-only anchor must keep its logical
            // slot. An anchor with surviving paint or other content remains.
            anchor = (|| {
                let index = self.replacement_anchor(&owner, &kids, page_id, tag).ok()?;
                let mcid = tag.mcid?;
                let empty = if tag.from_page {
                    self.empty_mark(&page_ops, &props, mcid)?
                } else {
                    let child = self.resolve_written(&kids[index]).ok()?.as_dict()?;
                    let form = self.resolve_written(child.get("Stm")?).ok()?;
                    let local = self.reference_dict(form.as_dict()?.get("Resources")).ok()?;
                    let form_props = self.reference_dict(local.get("Properties")).ok()?;
                    let data = form.decode_stream_data().ok()?;
                    if data.len() > MAX_CONTENT_BYTES {
                        return None;
                    }
                    let ops = parse_content_stream(&data).ok()?;
                    self.empty_mark(&ops, &form_props, mcid)?
                };
                empty.then_some(index)
            })();
        }
        let mut next = items.len();
        for op in page_ops {
            let Operator::BeginMarkedContentDict { properties, .. } = op else {
                continue;
            };
            let properties = match properties.as_name() {
                Some(name) => self.resolve_written(props.get(name).ok_or(INVALID)?)?,
                None => &properties,
            };
            if let Some(value) = properties.as_dict().and_then(|fields| fields.get("MCID")) {
                let current =
                    usize::try_from(self.resolve_written(value)?.as_integer().ok_or(INVALID)?)
                        .map_err(|_| INVALID)?;
                next = next.max(current.saturating_add(1));
            }
        }
        if next >= MAX_TAG_ITEMS {
            return Err(INVALID);
        }
        if anchor.is_some() && tag.retained_anchor.is_none() {
            let old = tag.mcid.ok_or(INVALID)?;
            if page_key == Some(tag.parent_key) {
                *items.get_mut(old).ok_or(INVALID)? = Object::Null;
            } else {
                let source = tree.entries.get(&tag.parent_key).ok_or(INVALID)?;
                let mut source = self
                    .resolve_written(source)?
                    .as_array()
                    .cloned()
                    .ok_or(INVALID)?;
                *source.get_mut(old).ok_or(INVALID)? = Object::Null;
                tree.entries.insert(tag.parent_key, Object::Array(source));
            }
        }
        items.resize(next, Object::Null);
        items.push(reference(owner_id));
        let continuation =
            if let Some(RetainedAnchor::InsertBetween { next_source_mcid }) = tag.retained_anchor {
                let first = anchor.ok_or(INVALID)?;
                let mut kid = dict([("Type", name("MCR")), ("Pg", reference(page_id))]);
                let mcid = if tag.from_page {
                    let mcid = items.len().max(next_source_mcid);
                    if mcid >= MAX_TAG_ITEMS {
                        return Err(INVALID);
                    }
                    let streams = self
                        .contents(&mut page.clone())?
                        .iter()
                        .map(|object| id(Some(object)).ok_or(INVALID))
                        .collect::<Result<Vec<_>>>()?;
                    self.write_continuation(&streams, mcid)?;
                    items.resize(mcid, Object::Null);
                    items.push(reference(owner_id));
                    mcid
                } else {
                    if Some(tag.parent_key) == page_key {
                        return Err(INVALID);
                    }
                    let source = self
                        .resolve_written(&kids[first])?
                        .as_dict()
                        .ok_or(INVALID)?;
                    let form = id(source.get("Stm")).ok_or(INVALID)?;
                    let mut source_slots = self
                        .resolve_written(tree.entries.get(&tag.parent_key).ok_or(INVALID)?)?
                        .as_array()
                        .cloned()
                        .ok_or(INVALID)?;
                    let mcid = source_slots.len().max(next_source_mcid);
                    if mcid >= MAX_TAG_ITEMS {
                        return Err(INVALID);
                    }
                    self.write_continuation(&[form], mcid)?;
                    source_slots.resize(mcid, Object::Null);
                    source_slots.push(reference(owner_id));
                    tree.entries
                        .insert(tag.parent_key, Object::Array(source_slots));
                    kid.insert("Stm".into(), reference(form));
                    mcid
                };
                kid.insert("MCID".into(), Object::Integer(mcid as i64));
                Some(Object::Dictionary(kid))
            } else {
                None
            };
        let items = Object::Array(items);
        if let Some(key) = page_key {
            tree.entries.insert(key, items);
        } else {
            page.insert("StructParents".into(), Object::Integer(tree.add(items)?));
        }
        let kid = Object::Dictionary(dict([
            ("Type", name("MCR")),
            ("Pg", reference(page_id)),
            ("MCID", Object::Integer(next as i64)),
        ]));
        if let Some(index) = anchor {
            match tag.retained_anchor {
                Some(RetainedAnchor::InsertBefore) => kids.insert(index, kid),
                Some(RetainedAnchor::InsertAfter) => kids.insert(index + 1, kid),
                Some(RetainedAnchor::InsertBetween { .. }) => {
                    kids.splice(index + 1..index + 1, [kid, continuation.ok_or(INVALID)?]);
                }
                None => kids[index] = kid,
            }
        } else {
            kids.push(kid);
        }
        owner.insert("K".into(), Object::Array(kids));
        self.set(owner_id, Object::Dictionary(owner));
        tree.write(self)?;
        Ok(next)
    }
}
