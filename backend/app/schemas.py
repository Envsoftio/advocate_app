from typing import Literal
from datetime import date
from pydantic import BaseModel, Field, ConfigDict, field_validator, model_validator
import json
import math
import re
from urllib.parse import urlparse
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

DocKind = Literal['petition', 'affidavit', 'order', 'annexure', 'application', 'evidence', 'other']
DocumentDateStatus = Literal['unknown', 'ambiguous', 'suggested', 'confirmed']
DocumentDateSource = Literal['none', 'user', 'extracted', 'filename', 'filesystem']
LabelStatus = Literal['unreviewed', 'confirmed']
LabelSource = Literal['none', 'legacy', 'user', 'proposal', 'filename']
GroupRole = Literal['member', 'parent']
VisibleSignatureStatus = Literal['unreviewed', 'present', 'absent']


class MatterCreate(BaseModel):
    name: str = Field(min_length=2, max_length=200)
    court_profile: str = 'general-demo'
    case_type: str = Field(default='Demonstration filing', max_length=150)
    petitioner: str = Field(default='', max_length=300)
    respondent: str = Field(default='', max_length=300)
    advocate: str = Field(default='', max_length=200)
    case_number: str = Field(default='', max_length=100)
    max_pages_per_volume: int = Field(default=300, ge=2, le=2000)
    keep_groups_intact: bool = True


class MatterPatch(BaseModel):
    model_config = ConfigDict(extra='forbid')
    name: str | None = Field(default=None, min_length=2, max_length=200)
    court_profile: str | None = None
    case_type: str | None = Field(default=None, max_length=150)
    petitioner: str | None = Field(default=None, max_length=300)
    respondent: str | None = Field(default=None, max_length=300)
    advocate: str | None = Field(default=None, max_length=200)
    case_number: str | None = Field(default=None, max_length=100)
    max_pages_per_volume: int | None = Field(default=None, ge=2, le=2000)
    keep_groups_intact: bool | None = None


class BundlePreviewRequest(BaseModel):
    model_config = ConfigDict(extra='forbid')
    max_pages_per_volume: int = Field(ge=2, le=2000)
    keep_groups_intact: bool


class DocumentPatch(BaseModel):
    model_config = ConfigDict(extra='forbid')
    title: str | None = Field(default=None, min_length=1, max_length=250)
    kind: DocKind | None = None
    part: int | None = Field(default=None, ge=1, le=4)
    label: str | None = Field(default=None, max_length=60)
    label_status: LabelStatus | None = None
    label_source: LabelSource | None = None
    group_key: str | None = Field(default=None, max_length=100)
    group_role: GroupRole | None = None
    document_date: str | None = Field(default=None, pattern=r'^(|\d{4}-\d{2}-\d{2})$')
    date_status: DocumentDateStatus | None = None
    date_source: DocumentDateSource | None = None
    date_note: str | None = Field(default=None, max_length=120)
    visible_signature_status: VisibleSignatureStatus | None = None

    @field_validator('document_date')
    @classmethod
    def validate_document_date(cls, value: str | None) -> str | None:
        if value:
            date.fromisoformat(value)
        return value

    @model_validator(mode='after')
    def validate_date_metadata(self):
        values = (self.document_date, self.date_status, self.date_source)
        if all(value is None for value in values) and self.date_note is None:
            return self
        if any(value is None for value in values):
            raise ValueError('Document date, status and source must be updated together')
        if self.date_status == 'unknown':
            if self.document_date or self.date_source != 'none' or self.date_note:
                raise ValueError('Unknown dates must not have a value or source')
        elif self.date_status == 'ambiguous':
            if self.document_date or self.date_source in ('none', 'user') or not self.date_note:
                raise ValueError('Ambiguous dates require a source and the unparsed date text')
        elif not self.document_date:
            raise ValueError('Suggested and confirmed dates require a date value')
        elif self.date_status == 'confirmed' and self.date_source != 'user':
            raise ValueError('Only a user-reviewed date can be confirmed')
        elif self.date_status == 'suggested' and self.date_source in ('none', 'user'):
            raise ValueError('Suggested dates require an extracted, filename or filesystem source')
        return self

    @model_validator(mode='after')
    def validate_label_metadata(self):
        if self.label_status is None and self.label_source is None:
            return self
        if self.label is None or self.label_status is None or self.label_source is None:
            raise ValueError('Label, confirmation status and source must be updated together')
        if not self.label:
            if self.label_status != 'unreviewed' or self.label_source != 'none':
                raise ValueError('An empty label cannot be confirmed or have a source')
        elif self.label_status == 'confirmed' and self.label_source not in ('user', 'proposal'):
            raise ValueError('Confirmed labels require a user or reviewed proposal source')
        elif self.label_status == 'unreviewed' and self.label_source not in ('legacy', 'filename'):
            raise ValueError('Unreviewed labels require a legacy or filename source')
        return self


class BatchDocumentPatch(BaseModel):
    model_config = ConfigDict(extra='forbid')
    ids: list[str] = Field(min_length=1, max_length=1000)
    kind: DocKind | None = None
    part: int | None = Field(default=None, ge=1, le=4)
    group_key: str | None = Field(default=None, max_length=100)

    @model_validator(mode='after')
    def require_change(self):
        if self.kind is None and self.part is None and self.group_key is None:
            raise ValueError('Choose at least one batch field')
        if len(self.ids) != len(set(self.ids)):
            raise ValueError('Select each document once')
        return self


class DocumentSortPreview(BaseModel):
    model_config = ConfigDict(extra='forbid')
    mode: Literal['manual', 'upload', 'filename', 'document_date']
    unknown_dates: Literal['last'] = 'last'


class LabelProposalRequest(BaseModel):
    model_config = ConfigDict(extra='forbid')
    ids: list[str] = Field(min_length=1, max_length=1000)
    mode: Literal['sequential', 'grouped_suffix', 'filename']
    prefix: str = Field(default='P-', max_length=20)
    start: int = Field(default=1, ge=1, le=9999)
    group_separator: str = Field(default='', max_length=5)
    group_suffix_style: Literal['lower_alpha', 'upper_alpha', 'numeric',
                                'lower_alpha_parenthesized',
                                'upper_alpha_parenthesized',
                                'numeric_parenthesized'] = 'lower_alpha'
    first_member_base: bool = False
    existing_label_policy: Literal['compact', 'preserve'] = 'compact'
    reference_document_id: str | None = None

    @field_validator('ids')
    @classmethod
    def unique_ids(cls, values: list[str]) -> list[str]:
        if len(values) != len(set(values)):
            raise ValueError('Select each document once')
        return values

    @field_validator('prefix', 'group_separator')
    @classmethod
    def visible_label_fragment(cls, value: str) -> str:
        if any(ord(char) < 32 for char in value):
            raise ValueError('Label formatting cannot contain control characters')
        return value


class LabelMapping(BaseModel):
    model_config = ConfigDict(extra='forbid')
    document_id: str
    old_label: str = Field(max_length=60)
    new_label: str = Field(min_length=1, max_length=60)


class ApplyLabelProposal(BaseModel):
    model_config = ConfigDict(extra='forbid')
    mappings: list[LabelMapping] = Field(min_length=1, max_length=1000)
    reference_document_id: str | None = None
    expected_reference_revision: int | None = Field(default=None, ge=1)
    acknowledge_existing_labels: bool = False
    acknowledge_prior_exports: bool = False
    acknowledge_references: bool = False

    @field_validator('mappings')
    @classmethod
    def unique_mapping_ids(cls, values: list[LabelMapping]) -> list[LabelMapping]:
        ids = [item.document_id for item in values]
        if len(ids) != len(set(ids)):
            raise ValueError('Map each document once')
        return values


class MemoParty(BaseModel):
    model_config = ConfigDict(extra='forbid')
    role: Literal['Petitioner', 'Respondent']
    name: str = Field(max_length=300)
    fatherOrHusbandName: str = Field(max_length=300)
    address: str = Field(max_length=1000)
    email: str = Field(max_length=300)
    mobile: str = Field(max_length=100)


class MemoPartiesUpdate(BaseModel):
    parties: list[MemoParty] = Field(min_length=1, max_length=100)


class MatterValuesUpdate(BaseModel):
    model_config = ConfigDict(extra='forbid')
    values: dict[str, str] = Field(max_length=100)

    @field_validator('values')
    @classmethod
    def validate_values(cls, values: dict[str, str]) -> dict[str, str]:
        for key, value in values.items():
            if not re.fullmatch(r'[a-z][a-z0-9_]{0,63}', key):
                raise ValueError('Matter value keys must be lowercase identifiers up to 64 characters')
            if len(value) > 2000:
                raise ValueError('Matter values must be at most 2000 characters')
        return values


class FilingReviewItem(BaseModel):
    model_config = ConfigDict(extra='forbid')
    title: str = Field(min_length=1, max_length=80)
    part: int | None = Field(default=None, ge=1, le=4)
    status: Literal['unreviewed', 'present', 'not_applicable'] = 'unreviewed'

    @field_validator('title')
    @classmethod
    def nonblank_title(cls, value: str) -> str:
        value = value.strip()
        if not value:
            raise ValueError('Checklist item title is required')
        return value


class FilingReview(BaseModel):
    """User-authored review aid, with no built-in court requirements."""
    model_config = ConfigDict(extra='forbid')
    court: str = Field(default='', max_length=120)
    case_type: str = Field(default='', max_length=120)
    stage: str = Field(default='', max_length=120)
    source_note: str = Field(default='', max_length=500)
    items: list[FilingReviewItem] = Field(default_factory=list, max_length=12)

    @model_validator(mode='after')
    def fits_existing_matter_values(self):
        import json
        if len(json.dumps([item.model_dump() for item in self.items], ensure_ascii=False)) > 2000:
            raise ValueError('Checklist is too long; shorten item titles')
        return self


class ReorderDocuments(BaseModel):
    ids: list[str]


class SignatureRiskAcknowledgement(BaseModel):
    acknowledge_signature_risk: bool = False

    @field_validator('acknowledge_signature_risk', mode='before')
    @classmethod
    def only_explicit_true_acknowledges(cls, value):
        # A missing or malformed UI value cannot grant signature-risk consent.
        return value is True


class OcrRequest(SignatureRiskAcknowledgement):
    expected_revision: int | None = Field(default=None, ge=1)
    language: Literal['eng', 'hin', 'eng+hin'] = 'eng'


class ReorderPages(SignatureRiskAcknowledgement):
    order: list[int]
    expected_revision: int | None = Field(default=None, ge=1, strict=True)


class BatchPageOperation(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    expected_revision: int = Field(ge=1, strict=True)
    operation: Literal['rotate', 'delete']
    pages: list[int] = Field(min_length=1, max_length=100000)
    degrees: int | None = Field(default=None, strict=True)

    @field_validator('pages', mode='before')
    @classmethod
    def explicit_page_indices(cls, value):
        if not isinstance(value, list) or any(type(index) is not int for index in value):
            raise ValueError('Page indexes must be integers')
        if len(value) != len(set(value)):
            raise ValueError('Select each page only once')
        return value

    @model_validator(mode='after')
    def validate_operation(self):
        if self.operation == 'rotate' and self.degrees not in (90, 180, 270):
            raise ValueError('Rotation must be 90, 180 or 270 degrees clockwise')
        if self.operation == 'delete' and self.degrees is not None:
            raise ValueError('Deletion does not accept a rotation')
        return self


class RotatePage(SignatureRiskAcknowledgement):
    page_index: int = Field(ge=0)
    degrees: int
    expected_revision: int | None = Field(default=None, ge=1, strict=True)


class DeletePage(SignatureRiskAcknowledgement):
    page_index: int = Field(ge=0)
    expected_revision: int | None = Field(default=None, ge=1, strict=True)


class InsertPages(SignatureRiskAcknowledgement):
    source_document_id: str
    expected_revision: int | None = Field(default=None, ge=1, strict=True)
    expected_source_revision: int | None = Field(default=None, ge=1, strict=True)
    source_pages: list[int] = Field(min_length=1, max_length=2000)
    before_index: int = Field(ge=0)


class ReplacePages(SignatureRiskAcknowledgement):
    source_document_id: str
    expected_revision: int | None = Field(default=None, ge=1, strict=True)
    expected_source_revision: int | None = Field(default=None, ge=1, strict=True)
    target_pages: list[int] = Field(min_length=1, max_length=2000)
    source_pages: list[int] = Field(min_length=1, max_length=2000)


class ReplaceDocument(SignatureRiskAcknowledgement):
    source_document_id: str
    expected_revision: int | None = Field(default=None, ge=1, strict=True)
    expected_source_revision: int | None = Field(default=None, ge=1, strict=True)


READER_KEY_PATTERN = r'^(?:original:[0-9a-f]{32}|[0-9a-f]{32}:[1-9][0-9]*|export:[0-9a-f]{32}:[0-9a-f]{32}:[A-Za-z0-9_-]+\.pdf)$'


class ReaderPosition(BaseModel):
    """Local navigation metadata; never PDF text, names or bytes."""
    model_config = ConfigDict(extra='forbid')
    key: str = Field(max_length=200, pattern=READER_KEY_PATTERN)
    page: int = Field(ge=1, le=100000, strict=True)
    x: float = Field(ge=0, le=1)
    y: float = Field(ge=0, le=1)
    zoom: Literal['fit-width', 'fit-page', '25', '50', '75', '100', '125', '150', '200', '300', '400']
    mode: Literal['single', 'continuous']


class CompressionRequest(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    preset: Literal['lossless', 'balanced', 'small'] = 'lossless'
    expected_revision: int = Field(ge=1)


class ManualBookmark(BaseModel):
    model_config = ConfigDict(extra='forbid')
    id: str = Field(pattern=r'^[0-9a-f]{32}$')
    title: str = Field(min_length=1, max_length=200)
    document_id: str
    page_index: int = Field(ge=0)
    target_revision: int = Field(ge=1)
    parent_id: str | None = None


class ManualBookmarksUpdate(BaseModel):
    expected_snapshot: str | None = Field(default=None, min_length=64, max_length=64)
    entries: list[ManualBookmark] = Field(max_length=500)


class ReviewedLink(BaseModel):
    """A user-reviewed PDF action anchored to a specific working revision."""
    model_config = ConfigDict(extra='forbid')
    id: str = Field(pattern=r'^[0-9a-f]{32}$')
    title: str = Field(min_length=1, max_length=200)
    source_document_id: str
    source_page_index: int = Field(ge=0)
    source_revision: int = Field(ge=1)
    rect_x: float = Field(ge=0, lt=1)
    rect_y: float = Field(ge=0, lt=1)
    rect_width: float = Field(gt=0, le=1)
    rect_height: float = Field(gt=0, le=1)
    kind: Literal['internal', 'external']
    target_document_id: str | None = None
    target_page_index: int | None = Field(default=None, ge=0)
    target_revision: int | None = Field(default=None, ge=1)
    uri: str = Field(default='', max_length=2000)

    @model_validator(mode='after')
    def validate_target(self):
        if self.rect_x + self.rect_width > 1 or self.rect_y + self.rect_height > 1:
            raise ValueError('Link region must stay within the displayed page')
        if self.kind == 'internal':
            if (not self.target_document_id or self.target_page_index is None or
                    self.target_revision is None or self.uri):
                raise ValueError('Internal links require one current document page and no URL')
        else:
            if (self.target_document_id is not None or self.target_page_index is not None or
                    self.target_revision is not None):
                raise ValueError('External links cannot also have an internal target')
            parsed = urlparse(self.uri)
            if parsed.scheme.lower() not in ('https', 'http', 'mailto'):
                raise ValueError('External links support only https, http or mailto destinations')
            if parsed.scheme.lower() in ('https', 'http') and not parsed.netloc:
                raise ValueError('External web links require a host name')
            if parsed.scheme.lower() == 'mailto' and not parsed.path:
                raise ValueError('Mail links require an address')
        return self


class ReviewedLinksUpdate(BaseModel):
    entries: list[ReviewedLink] = Field(max_length=500)


MarkPosition = Literal['top_left', 'top_center', 'top_right', 'center',
                       'bottom_left', 'bottom_center', 'bottom_right']


class VolumeSplit(BaseModel):
    model_config = ConfigDict(extra='forbid')
    revision: int = Field(ge=1)
    after_pages: list[int] = Field(min_length=1, max_length=2000)

    @field_validator('after_pages')
    @classmethod
    def ordered_boundaries(cls, value):
        if value != sorted(set(value)) or any(page < 1 for page in value):
            raise ValueError('Split boundaries must be positive, unique and increasing')
        return value


class OutputMarks(BaseModel):
    """Matter-level technical output marks. This is not a court-rule preset."""
    model_config = ConfigDict(extra='forbid')
    volume_max_bytes: int | None = Field(default=None, ge=1, le=10_000_000_000)
    volume_splits: dict[str, VolumeSplit] = Field(default_factory=dict, max_length=1000)
    processing_quality: Literal['original', 'lossless', 'balanced', 'small'] = 'original'
    cleanup_metadata: bool = False
    print_layout: Literal['screen'] = 'screen'
    office_docx: bool = False
    office_xlsx: bool = False
    office_pptx: bool = False
    numbering_style: Literal['arabic', 'roman_lower', 'roman_upper'] = 'arabic'
    numbering_restart: Literal['volume', 'continuous', 'section'] = 'volume'
    numbering_start: int = Field(default=1, ge=1, le=3999)
    numbering_prefix: str = Field(default='', max_length=24)
    numbering_position: Literal['bottom_left', 'bottom_center', 'bottom_right', 'top_left', 'top_right'] = 'bottom_center'
    numbering_front_matter: Literal['physical', 'roman', 'none'] = 'physical'
    numbering_pdf_labels: bool = True
    numbering_linked_index: bool = True
    numbering_back_link: bool = False
    index_style: Literal['compact', 'readable', 'review'] = 'compact'
    front_matter: Literal['index_only', 'cover_index'] = 'index_only'
    index_date: bool = False
    cover_heading: str = Field(default='', max_length=200)
    cover_reference: str = Field(default='', max_length=100)
    cover_title: str = Field(default='', max_length=200)
    part_extracts_enabled: bool = False
    watermark_enabled: bool = False
    watermark_text: str = Field(default='DRAFT', max_length=120)
    watermark_position: MarkPosition = 'center'
    watermark_font_size: float = Field(default=32, ge=8, le=72)
    watermark_opacity: float = Field(default=.16, ge=.05, le=.8)
    timestamp_enabled: bool = False
    timestamp_format: Literal['date', 'datetime'] = 'datetime'
    timestamp_timezone: str = Field(default='UTC', min_length=1, max_length=64)
    timestamp_position: MarkPosition = 'bottom_right'
    timestamp_font_size: float = Field(default=8, ge=6, le=24)
    timestamp_prefix: str = Field(default='Built', max_length=40)
    bates_enabled: bool = False
    bates_prefix: str = Field(default='', max_length=24)
    bates_start: int = Field(default=1, ge=1, le=999999999)
    bates_padding: int = Field(default=6, ge=1, le=12)
    bates_position: MarkPosition = 'bottom_left'
    bates_font_size: float = Field(default=8, ge=6, le=24)
    bates_include_index: bool = False
    bates_include_separator: bool = False
    bates_restart_per_volume: bool = False
    annexure_mode: Literal['none', 'label_stamp', 'separator'] = 'none'
    annexure_stamp_scope: Literal['first', 'all'] = 'first'
    annexure_stamp_position: MarkPosition = 'top_right'
    annexure_stamp_font_size: float = Field(default=10, ge=6, le=24)
    annexure_overrides: dict[str, Literal['none', 'label_stamp', 'separator']] = Field(
        default_factory=dict, max_length=1000)

    @field_validator('watermark_text', 'timestamp_prefix', 'bates_prefix')
    @classmethod
    def printable_ascii(cls, value: str) -> str:
        if any(ord(char) < 32 or ord(char) > 126 for char in value):
            raise ValueError('Output marks currently support printable ASCII only')
        return value

    @field_validator('numbering_prefix')
    @classmethod
    def valid_numbering_prefix(cls, value: str) -> str:
        remainder = value.replace('{volume}', '').replace('{section}', '')
        if '{' in remainder or '}' in remainder or any(ord(c) < 32 or ord(c) > 255 for c in value):
            raise ValueError('Use Latin-1 text and only {volume} or {section} placeholders')
        return value

    @field_validator('volume_splits')
    @classmethod
    def valid_split_ids(cls, values):
        if any(not re.fullmatch(r'[0-9a-f]{32}', key) for key in values):
            raise ValueError('Splits require valid document IDs')
        return values

    @field_validator('annexure_overrides')
    @classmethod
    def valid_override_ids(cls, values: dict[str, str]) -> dict[str, str]:
        if any(not re.fullmatch(r'[0-9a-f]{32}', key) for key in values):
            raise ValueError('Annexure exceptions require valid document IDs')
        return values

    @field_validator('timestamp_timezone')
    @classmethod
    def valid_timezone(cls, value: str) -> str:
        try:
            ZoneInfo(value)
        except (ZoneInfoNotFoundError, ValueError) as exc:
            raise ValueError('Use an installed IANA timezone such as UTC or Asia/Kolkata') from exc
        return value

    @model_validator(mode='after')
    def require_mark_text(self):
        if self.watermark_enabled and not self.watermark_text.strip():
            raise ValueError('Enabled watermark requires visible text')
        return self


class OutputPresetCreate(BaseModel):
    """A reusable technical snapshot. Versions are immutable once saved."""
    model_config = ConfigDict(extra='forbid')
    name: str = Field(min_length=1, max_length=80)
    max_pages_per_volume: int = Field(ge=2, le=2000)
    keep_groups_intact: bool = True
    output_marks: OutputMarks = Field(default_factory=OutputMarks)

    @field_validator('name')
    @classmethod
    def nonblank_name(cls, value: str) -> str:
        value = value.strip()
        if not value:
            raise ValueError('Preset name is required')
        return value

    @model_validator(mode='after')
    def no_matter_specific_documents(self):
        if any((self.output_marks.cover_heading, self.output_marks.cover_reference, self.output_marks.cover_title)):
            raise ValueError('Reusable presets cannot contain matter-specific cover details')
        if self.output_marks.volume_splits:
            raise ValueError('Reusable presets cannot contain document split boundaries')
        if self.output_marks.annexure_overrides:
            raise ValueError('Reusable presets cannot contain matter-specific document exceptions')
        return self


class BundleSettingsUpdate(BundlePreviewRequest):
    output_marks: OutputMarks
    expected_snapshot: str = Field(min_length=64, max_length=64)


class FrontMatterPreview(BundlePreviewRequest):
    output_marks: OutputMarks
    volume: int = Field(default=1, ge=1)
    index_page: int = Field(default=1, ge=1)


class NumberingPreview(FrontMatterPreview):
    document_id: str
    page: int = Field(default=1, ge=1)
    expected_revision: int = Field(ge=1)


class PresetApplication(BaseModel):
    model_config = ConfigDict(extra='forbid')
    preserve_overrides: bool = False
    expected_snapshot: str | None = Field(default=None, min_length=64, max_length=64)


class CropPages(SignatureRiskAcknowledgement):
    pages: list[int] = Field(min_length=1, max_length=2000)
    expected_revision: int | None = Field(default=None, ge=1, strict=True)
    left: float = Field(ge=0, le=1)
    top: float = Field(ge=0, le=1)
    right: float = Field(ge=0, le=1)
    bottom: float = Field(ge=0, le=1)


class TextOverlay(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    x: float = Field(ge=0, lt=1)
    y: float = Field(ge=0, lt=1)
    text: str = Field(min_length=1, max_length=1000)
    font_size: float = Field(default=12, ge=6, le=36)


AnnotationColor = Literal['yellow', 'green', 'blue', 'pink']
ReviewMarkKind = Literal['highlight', 'comment', 'draw', 'box', 'ellipse', 'line', 'arrow',
                         'check', 'cross', 'date', 'text', 'redaction']


def valid_date_mark(body: str) -> bool:
    if not re.fullmatch(r'\d{4}-\d{2}-\d{2}', body):
        return False
    try:
        date.fromisoformat(body)
        return True
    except ValueError:
        return False


def valid_draw_body(body: str) -> bool:
    try:
        data = json.loads(body)
    except (TypeError, ValueError):
        return False
    if not isinstance(data, dict) or set(data) != {'v', 'points'} or type(data['v']) is not int or data['v'] != 1:
        return False
    points = data['points']
    return (isinstance(points, list) and 2 <= len(points) <= 160 and
            all(isinstance(point, list) and len(point) == 2 and
                all(type(value) in (int, float) and math.isfinite(value) and 0 <= value <= 1
                    for value in point) for point in points))


class PageAnnotationCreate(BaseModel):
    """An editable page mark anchored to one exact working revision."""
    model_config = ConfigDict(extra='forbid')
    kind: ReviewMarkKind
    page_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    rect_x: float = Field(ge=0, lt=1)
    rect_y: float = Field(ge=0, lt=1)
    rect_width: float = Field(gt=0, le=1)
    rect_height: float = Field(gt=0, le=1)
    body: str = Field(default='', max_length=4000)
    color: AnnotationColor = 'yellow'
    output_scope: Literal['private', 'pdf'] = 'private'

    @model_validator(mode='after')
    def validate_annotation(self):
        if self.rect_x + self.rect_width > 1 or self.rect_y + self.rect_height > 1:
            raise ValueError('Annotation region must stay within the displayed page')
        if self.kind == 'comment' and not self.body.strip():
            raise ValueError('Comments require visible note text')
        if self.kind == 'date' and not valid_date_mark(self.body):
            raise ValueError('Date marks require a YYYY-MM-DD label')
        if self.kind == 'draw' and not valid_draw_body(self.body):
            raise ValueError('Draw marks require a bounded point path')
        if self.kind == 'text' and (not self.body.strip() or any(
                ord(char) < 32 and char != '\n' or ord(char) > 126 for char in self.body)):
            raise ValueError('Text marks require printable ASCII text')
        if self.kind == 'redaction' and (self.output_scope != 'pdf' or self.body):
            raise ValueError('Redaction marks require PDF output scope and no text')
        if self.kind in ('highlight', 'comment') and self.output_scope != 'private':
            raise ValueError('Review highlights and comments must remain private')
        return self


class PageAnnotationUpdate(BaseModel):
    model_config = ConfigDict(extra='forbid')
    expected_revision: int = Field(ge=1)
    rect_x: float = Field(ge=0, lt=1)
    rect_y: float = Field(ge=0, lt=1)
    rect_width: float = Field(gt=0, le=1)
    rect_height: float = Field(gt=0, le=1)
    body: str = Field(default='', max_length=4000)
    color: AnnotationColor = 'yellow'
    output_scope: Literal['private', 'pdf'] | None = None

    @model_validator(mode='after')
    def validate_region(self):
        if self.rect_x + self.rect_width > 1 or self.rect_y + self.rect_height > 1:
            raise ValueError('Annotation region must stay within the displayed page')
        return self


class ReplaceTextRun(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    span_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    expected_text: str = Field(min_length=1, max_length=2000)
    replacement: str = Field(min_length=1, max_length=2000)
    expected_paragraph: str | None = Field(default=None, max_length=4000)


class ParagraphSelection(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    span_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    expected_text: str = Field(min_length=1, max_length=2000)


class MoveTextRun(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    span_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    expected_text: str = Field(min_length=1, max_length=2000)
    horizontal_offset: float = Field(default=0, ge=-1, le=1)
    vertical_offset: float = Field(ge=-1, le=1)


class PermanentRedaction(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    rect_x: float = Field(ge=0, lt=1)
    rect_y: float = Field(ge=0, lt=1)
    rect_width: float = Field(gt=0, le=1)
    rect_height: float = Field(gt=0, le=1)

    @model_validator(mode='after')
    def validate_region(self):
        if self.rect_x + self.rect_width > 1 or self.rect_y + self.rect_height > 1:
            raise ValueError('Redaction region must stay within the displayed page')
        return self


class RestoreRedaction(SignatureRiskAcknowledgement):
    model_config = ConfigDict(extra='forbid')
    expected_revision: int = Field(ge=1)


class JobView(BaseModel):
    id: str
    matter_id: str
    kind: str
    status: str
    progress: int
    message: str
    error: str | None = None
    result: dict | None = None


class Check(BaseModel):
    severity: Literal['info', 'warning', 'error']
    code: str
    message: str
    document_id: str | None = None


class BundleMembershipUpdate(BaseModel):
    included_ids: list[str] = Field(max_length=10000)
    expected_snapshot: str = Field(min_length=64, max_length=64)


class SupplementalPlanRequest(BundlePreviewRequest):
    model_config = ConfigDict(extra='forbid')
    baseline_build_id: str = Field(pattern=r'^[0-9a-f]{32}$')
    title: str = Field(min_length=1, max_length=200)
    selected_page_ids: list[str] = Field(default_factory=list, max_length=100000)
    output_marks: OutputMarks
    expected_snapshot: str = Field(min_length=64, max_length=64)
    expected_version: int | None = Field(default=None, ge=1)
    insertion_policy: Literal['separate_supplement', 'preserve_printed_labels'] | None = None
    policy_reviewed: bool = False
