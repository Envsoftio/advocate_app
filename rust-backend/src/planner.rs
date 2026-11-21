//! Deterministic page map for a frozen, already selected set of working revisions.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Deserialize)]
pub struct Document {
    id: String,
    title: String,
    part: u32,
    kind: String,
    label: String,
    document_date: Option<String>,
    revision: Option<u32>,
    page_count: usize,
    group_key: String,
    byte_size: u64,
    #[serde(default, rename = "_source_start")]
    source_start: usize,
    #[serde(default, rename = "_source_total")]
    source_total: Option<usize>,
}

#[derive(Deserialize)]
struct VolumeSplit {
    revision: u32,
    after_pages: Vec<usize>,
}

#[derive(Deserialize)]
pub struct Bookmark {
    id: String,
    title: String,
    resolved: bool,
    document_id: String,
    page_index: usize,
}

#[derive(Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum IndexStyle {
    #[default]
    Compact,
    Readable,
    Review,
}

#[derive(Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum FrontMatter {
    #[default]
    IndexOnly,
    CoverIndex,
}

#[derive(Default, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AnnexureMode {
    #[default]
    None,
    LabelStamp,
    Separator,
}

#[derive(Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum NumberingStyle {
    #[default]
    Arabic,
    RomanLower,
    RomanUpper,
}

#[derive(Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum NumberingRestart {
    #[default]
    Volume,
    Continuous,
    Section,
}

#[derive(Default, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum FrontNumbering {
    #[default]
    Physical,
    Roman,
    None,
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Settings {
    volume_splits: HashMap<String, VolumeSplit>,
    front_matter: FrontMatter,
    index_date: bool,
    index_style: IndexStyle,
    numbering_start: usize,
    numbering_prefix: String,
    numbering_style: NumberingStyle,
    numbering_restart: NumberingRestart,
    numbering_front_matter: FrontNumbering,
    numbering_pdf_labels: bool,
    annexure_mode: AnnexureMode,
    annexure_overrides: HashMap<String, AnnexureMode>,
    part_extracts_enabled: bool,
    bates_enabled: bool,
    bates_start: usize,
    bates_prefix: String,
    bates_padding: usize,
    bates_include_index: bool,
    bates_include_separator: bool,
    bates_restart_per_volume: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume_splits: HashMap::new(),
            front_matter: FrontMatter::default(),
            index_date: false,
            index_style: IndexStyle::default(),
            numbering_start: 1,
            numbering_prefix: String::new(),
            numbering_style: NumberingStyle::default(),
            numbering_restart: NumberingRestart::default(),
            numbering_front_matter: FrontNumbering::default(),
            numbering_pdf_labels: true,
            annexure_mode: AnnexureMode::default(),
            annexure_overrides: HashMap::new(),
            part_extracts_enabled: false,
            bates_enabled: false,
            bates_start: 1,
            bates_prefix: String::new(),
            bates_padding: 6,
            bates_include_index: false,
            bates_include_separator: false,
            bates_restart_per_volume: false,
        }
    }
}

impl Settings {
    fn index_pages(&self, rows: usize) -> usize {
        let capacity = match (self.index_style, self.index_date) {
            (IndexStyle::Review, _) => 8,
            (IndexStyle::Readable, _) | (_, true) => 16,
            _ => 23,
        };
        rows.div_ceil(capacity).max(1)
    }

    fn cover_pages(&self) -> usize {
        usize::from(self.front_matter == FrontMatter::CoverIndex)
    }

    fn annexure_mode(&self, doc: &Document) -> AnnexureMode {
        if doc.kind == "annexure" {
            self.annexure_overrides
                .get(&doc.id)
                .copied()
                .unwrap_or(self.annexure_mode)
        } else {
            AnnexureMode::None
        }
    }
}

#[derive(Serialize)]
pub struct BundleMap<'a> {
    pub volumes: Vec<Volume<'a>>,
    pub combined_pages: usize,
    pub source_pages: usize,
    pub generated_pages: usize,
    pub included_documents: usize,
    pub manual_bookmarks: Vec<BookmarkDestination<'a>>,
    pub part_extracts: Vec<PartExtract>,
    pub bates_policy: BatesPolicy,
    pub numbering: &'static str,
}

#[derive(Serialize)]
pub struct Volume<'a> {
    pub estimated_bytes: u64,
    pub number: usize,
    pub index_pages: usize,
    pub cover_pages: usize,
    pub page_count: usize,
    pub combined_start_page: usize,
    pub combined_end_page: usize,
    pub rows: Vec<Row<'a>>,
    pub pages: Vec<Page<'a>>,
    pub bates_labels: Vec<Option<String>>,
}

#[derive(Serialize)]
pub struct Row<'a> {
    pub id: &'a str,
    pub part: u32,
    pub kind: &'a str,
    pub label: &'a str,
    pub title: &'a str,
    pub document_date: Option<&'a str>,
    pub revision: Option<u32>,
    pub source_start: usize,
    pub source_end: usize,
    pub start_page: usize,
    pub end_page: usize,
    pub volume: usize,
    pub total_pages: usize,
    pub annexure_mode: AnnexureMode,
    pub separator_page: Option<usize>,
    pub printed_start: String,
    pub printed_end: String,
    pub combined_start_page: usize,
    pub combined_end_page: usize,
    pub bates_first: Option<String>,
    pub bates_last: Option<String>,
    pub separator_bates: Option<String>,
}

#[derive(Serialize)]
pub struct Page<'a> {
    pub physical_page: usize,
    pub combined_page: usize,
    pub printed_label: String,
    pub pdf_label: String,
    pub document_id: Option<&'a str>,
    pub page_index: Option<usize>,
    pub revision: Option<u32>,
}

#[derive(Serialize)]
pub struct BookmarkDestination<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub resolved: bool,
    pub document_title: &'a str,
    pub document_page: usize,
    pub volume: Option<usize>,
    pub printed_label: Option<String>,
    pub volume_page: Option<usize>,
    pub combined_page: Option<usize>,
}

#[derive(Serialize)]
pub struct PartExtract {
    pub part: u32,
    pub documents: usize,
    pub pages: usize,
}

#[derive(Serialize)]
pub struct BatesPolicy {
    pub enabled: bool,
    pub include_index: bool,
    pub include_separator: bool,
    pub restart_per_volume: bool,
}

struct SourceRun<'a> {
    document: &'a Document,
    start: usize,
    end: usize,
    force_break: bool,
}

impl SourceRun<'_> {
    fn pages(&self) -> usize {
        self.end - self.start
    }
}

fn groups(documents: &[Document], keep_groups: bool) -> Result<Vec<Vec<&Document>>, String> {
    let mut seen = HashSet::new();
    let mut previous = "";
    let mut groups: Vec<Vec<&Document>> = Vec::new();
    for doc in documents {
        let key = if keep_groups {
            doc.group_key.trim()
        } else {
            ""
        };
        if key.is_empty() || key != previous {
            if !key.is_empty() && !seen.insert(key) {
                return Err(format!("Group \"{key}\" is not contiguous. Reorder documents so grouped records are adjacent."));
            }
            groups.push(Vec::new());
        }
        groups.last_mut().expect("group created above").push(doc);
        previous = key;
    }
    Ok(groups)
}

pub(crate) fn validate_splits(
    documents: &[Document],
    keep_groups: bool,
    settings: &Settings,
) -> Result<(), String> {
    split_units(documents, keep_groups, settings).map(|_| ())
}

fn split_units<'a>(
    documents: &'a [Document],
    keep_groups: bool,
    settings: &Settings,
) -> Result<Vec<Vec<SourceRun<'a>>>, String> {
    let mut units = Vec::new();
    for group in groups(documents, keep_groups)? {
        let mut pending = Vec::new();
        for document in group {
            let Some(split) = settings.volume_splits.get(&document.id) else {
                pending.push(SourceRun {
                    document,
                    start: document.source_start,
                    end: document.source_start + document.page_count,
                    force_break: false,
                });
                continue;
            };
            if Some(split.revision) != document.revision {
                return Err(format!(
                    "Split review is stale for {}; review its current revision",
                    document.title
                ));
            }
            let cuts = &split.after_pages;
            if cuts.is_empty()
                || cuts[0] == 0
                || cuts[cuts.len() - 1] >= document.page_count
                || cuts.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(format!(
                    "Invalid internal split boundary for {}",
                    document.title
                ));
            }
            if !pending.is_empty() {
                units.push(std::mem::take(&mut pending));
            }
            let mut start = 0;
            for end in cuts.iter().copied().chain([document.page_count]) {
                units.push(vec![SourceRun {
                    document,
                    start,
                    end,
                    force_break: start > 0,
                }]);
                start = end;
            }
        }
        if !pending.is_empty() {
            units.push(pending);
        }
    }
    Ok(units)
}

fn volume_plan<'a>(
    documents: &'a [Document],
    max_pages: usize,
    keep_groups: bool,
    settings: &Settings,
) -> Result<Vec<Vec<SourceRun<'a>>>, String> {
    let mut volumes = Vec::new();
    let mut current = Vec::new();
    let mut page_count = 0;
    for unit in split_units(documents, keep_groups, settings)? {
        let unit_pages = unit.iter().try_fold(0usize, |pages, run| {
            let separator = settings.annexure_mode(run.document) == AnnexureMode::Separator;
            pages
                .checked_add(run.pages())?
                .checked_add(usize::from(separator))
        });
        let unit_front = settings.index_pages(unit.len()) + settings.cover_pages();
        let Some(unit_pages) =
            unit_pages.filter(|pages| *pages <= max_pages.saturating_sub(unit_front))
        else {
            let titles: Vec<_> = unit.iter().map(|run| run.document.title.as_str()).collect();
            return Err(format!("Document/group ({}) exceeds {max_pages}-page volume cap including index. No document was split. Increase the cap or revise the grouping policy.", titles.join(", ")));
        };
        let next_front = settings.index_pages(current.len() + unit.len()) + settings.cover_pages();
        if !current.is_empty()
            && (unit[0].force_break || page_count + unit_pages + next_front > max_pages)
        {
            volumes.push(std::mem::take(&mut current));
            page_count = 0;
        }
        current.extend(unit);
        page_count += unit_pages;
    }
    if !current.is_empty() {
        volumes.push(current);
    }
    Ok(volumes)
}

fn roman(mut number: usize) -> String {
    let mut result = String::new();
    for (value, symbol) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        result.push_str(&symbol.repeat(number / value));
        number %= value;
    }
    result
}

struct Numbering<'a> {
    settings: &'a Settings,
    next_printed: usize,
    next_bates: usize,
    previous_part: Option<u32>,
}

impl Numbering<'_> {
    fn start_volume(&mut self) {
        if self.settings.numbering_restart == NumberingRestart::Volume {
            self.next_printed = self.settings.numbering_start;
        }
        if self.settings.bates_restart_per_volume {
            self.next_bates = self.settings.bates_start;
        }
    }

    fn print(&mut self, volume: usize, part: Option<u32>) -> String {
        if let Some(part) = part {
            if self.settings.numbering_restart == NumberingRestart::Section
                && self.previous_part != Some(part)
            {
                self.next_printed = self.settings.numbering_start;
            }
            self.previous_part = Some(part);
        }
        let number = self.next_printed;
        self.next_printed += 1;
        let label = match self.settings.numbering_style {
            NumberingStyle::Arabic => number.to_string(),
            NumberingStyle::RomanLower => roman(number).to_ascii_lowercase(),
            NumberingStyle::RomanUpper => roman(number),
        };
        self.settings
            .numbering_prefix
            .replace("{volume}", &volume.to_string())
            .replace(
                "{section}",
                &part.map(|part| part.to_string()).unwrap_or_default(),
            )
            + &label
    }

    fn bates(&mut self, included: bool) -> Option<String> {
        if !included || !self.settings.bates_enabled {
            return None;
        }
        let number = self.next_bates;
        self.next_bates += 1;
        Some(format!(
            "{}{number:0width$}",
            self.settings.bates_prefix,
            width = self.settings.bates_padding
        ))
    }
}

fn make_volume<'a>(
    runs: &[SourceRun<'a>],
    number: usize,
    offset: usize,
    numbering: &mut Numbering,
) -> Volume<'a> {
    let settings = numbering.settings;
    let cover_pages = settings.cover_pages();
    let index_pages = settings.index_pages(runs.len());
    let front_pages = cover_pages + index_pages;
    let mut rows = Vec::with_capacity(runs.len());
    let mut pages = Vec::new();
    let mut bates_labels = Vec::new();
    let mut estimated_bytes = front_pages as u64 * 2048;
    numbering.start_volume();
    for physical_page in 1..=front_pages {
        let (printed_label, pdf_label) = match settings.numbering_front_matter {
            FrontNumbering::Physical => {
                let label = numbering.print(number, None);
                (label.clone(), label)
            }
            FrontNumbering::Roman => (String::new(), roman(physical_page).to_ascii_lowercase()),
            FrontNumbering::None => (String::new(), String::new()),
        };
        pages.push(Page {
            physical_page,
            combined_page: offset + physical_page,
            printed_label,
            pdf_label: if settings.numbering_pdf_labels {
                pdf_label
            } else {
                physical_page.to_string()
            },
            document_id: None,
            page_index: None,
            revision: None,
        });
        bates_labels
            .push(numbering.bates(physical_page > cover_pages && settings.bates_include_index));
    }
    for run in runs {
        let doc = run.document;
        let mode = settings.annexure_mode(doc);
        let separator = mode == AnnexureMode::Separator;
        let separator_page = separator.then_some(pages.len() + 1);
        let start_page = pages.len() + 1 + usize::from(separator);
        let end_page = start_page + run.pages() - 1;
        for physical_page in pages.len() + 1..=end_page {
            let source_page = physical_page
                .checked_sub(start_page)
                .map(|index| run.start + index);
            let printed_label = numbering.print(number, Some(doc.part));
            let pdf_label = if settings.numbering_pdf_labels {
                printed_label.clone()
            } else {
                physical_page.to_string()
            };
            pages.push(Page {
                physical_page,
                combined_page: offset + physical_page,
                printed_label,
                pdf_label,
                document_id: source_page.map(|_| doc.id.as_str()),
                page_index: source_page,
                revision: source_page.and(doc.revision),
            });
            bates_labels
                .push(numbering.bates(source_page.is_some() || settings.bates_include_separator));
        }
        estimated_bytes += (doc.byte_size as f64 * run.pages() as f64
            / doc.source_total.unwrap_or(doc.page_count) as f64)
            .round_ties_even() as u64;
        if separator {
            estimated_bytes += 2048;
        }
        rows.push(Row {
            id: &doc.id,
            part: doc.part,
            kind: &doc.kind,
            label: &doc.label,
            title: &doc.title,
            document_date: doc.document_date.as_deref(),
            revision: doc.revision,
            source_start: run.start,
            source_end: run.end,
            start_page,
            end_page,
            volume: number,
            total_pages: run.pages(),
            annexure_mode: mode,
            separator_page,
            printed_start: pages[start_page - 1].printed_label.clone(),
            printed_end: pages[end_page - 1].printed_label.clone(),
            combined_start_page: offset + start_page,
            combined_end_page: offset + end_page,
            bates_first: bates_labels[start_page - 1].clone(),
            bates_last: bates_labels[end_page - 1].clone(),
            separator_bates: separator_page.and_then(|page| bates_labels[page - 1].clone()),
        });
    }
    Volume {
        estimated_bytes,
        number,
        index_pages,
        cover_pages,
        page_count: pages.len(),
        combined_start_page: offset + 1,
        combined_end_page: offset + pages.len(),
        rows,
        pages,
        bates_labels,
    }
}

/// Produce the Python `bundle_preview` map from validated settings and frozen metadata.
/// Source page indices are zero-based. Physical volume and combined positions are one-based.
pub fn plan_bundle<'a>(
    documents: &'a [Document],
    document_titles: &'a HashMap<String, String>,
    bookmarks: &'a [Bookmark],
    max_pages: usize,
    keep_groups: bool,
    settings: &Settings,
) -> Result<BundleMap<'a>, String> {
    if !(2..=2000).contains(&max_pages)
        || !(1..=12).contains(&settings.bates_padding)
        || !(1..=3999).contains(&settings.numbering_start)
        || !(1..=999_999_999).contains(&settings.bates_start)
    {
        return Err("Invalid page map settings".into());
    }
    if documents.iter().any(|doc| {
        doc.page_count == 0
            || doc
                .source_start
                .checked_add(doc.page_count)
                .is_none_or(|end| end > doc.source_total.unwrap_or(doc.page_count))
    }) {
        return Err("Invalid source page range".into());
    }
    let mut numbering = Numbering {
        settings,
        next_printed: settings.numbering_start,
        next_bates: settings.bates_start,
        previous_part: None,
    };
    let mut volumes = Vec::new();
    let mut combined_pages = 0;
    for runs in volume_plan(documents, max_pages, keep_groups, settings)? {
        let volume = make_volume(&runs, volumes.len() + 1, combined_pages, &mut numbering);
        combined_pages = volume.combined_end_page;
        volumes.push(volume);
    }

    let mut destinations = HashMap::new();
    let mut parts = BTreeMap::new();
    for volume in &volumes {
        for page in &volume.pages {
            if let (Some(id), Some(index)) = (page.document_id, page.page_index) {
                destinations.insert((id, index), (volume.number, page));
            }
        }
        if settings.part_extracts_enabled {
            for row in &volume.rows {
                let extract = parts.entry(row.part).or_insert(PartExtract {
                    part: row.part,
                    documents: 0,
                    pages: 0,
                });
                extract.documents += 1;
                extract.pages += row.total_pages;
            }
        }
    }
    let manual_bookmarks = bookmarks
        .iter()
        .map(|bookmark| {
            let destination = destinations
                .get(&(bookmark.document_id.as_str(), bookmark.page_index))
                .filter(|_| bookmark.resolved);
            BookmarkDestination {
                id: &bookmark.id,
                title: &bookmark.title,
                resolved: destination.is_some(),
                document_title: document_titles
                    .get(&bookmark.document_id)
                    .map(String::as_str)
                    .unwrap_or("Archived document"),
                document_page: bookmark.page_index + 1,
                volume: destination.map(|(volume, _)| *volume),
                printed_label: destination.map(|(_, page)| page.printed_label.clone()),
                volume_page: destination.map(|(_, page)| page.physical_page),
                combined_page: destination.map(|(_, page)| page.combined_page),
            }
        })
        .collect();
    let source_pages = documents.iter().map(|document| document.page_count).sum();
    Ok(BundleMap { volumes, combined_pages, source_pages, generated_pages: combined_pages - source_pages,
        included_documents: documents.len(), manual_bookmarks, part_extracts: parts.into_values().collect(),
        bates_policy: BatesPolicy { enabled: settings.bates_enabled, include_index: settings.bates_include_index,
            include_separator: settings.bates_include_separator, restart_per_volume: settings.bates_restart_per_volume },
        numbering: "Printed labels and PDF labels are distinct from physical volume and combined positions; all destinations use this engine map." })
}
