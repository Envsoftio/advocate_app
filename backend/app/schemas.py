from typing import Literal
from pydantic import BaseModel, Field, ConfigDict, field_validator
import re

DocKind = Literal['petition', 'affidavit', 'order', 'annexure', 'application', 'evidence', 'other']


class MatterCreate(BaseModel):
    name: str = Field(min_length=2, max_length=200)
    court_profile: str = 'delhi-hc-illustrative'
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


class DocumentPatch(BaseModel):
    model_config = ConfigDict(extra='forbid')
    title: str | None = Field(default=None, min_length=1, max_length=250)
    kind: DocKind | None = None
    part: int | None = Field(default=None, ge=1, le=4)
    label: str | None = Field(default=None, max_length=60)
    group_key: str | None = Field(default=None, max_length=100)


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


class ReorderDocuments(BaseModel):
    ids: list[str]


class ReorderPages(BaseModel):
    order: list[int]


class RotatePage(BaseModel):
    page_index: int = Field(ge=0)
    degrees: int


class DeletePage(BaseModel):
    page_index: int = Field(ge=0)


class InsertPages(BaseModel):
    source_document_id: str
    source_pages: list[int] = Field(min_length=1, max_length=2000)
    before_index: int = Field(ge=0)


class ReplacePages(BaseModel):
    source_document_id: str
    target_pages: list[int] = Field(min_length=1, max_length=2000)
    source_pages: list[int] = Field(min_length=1, max_length=2000)


class ReplaceDocument(BaseModel):
    source_document_id: str


class CompressionRequest(BaseModel):
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
    entries: list[ManualBookmark] = Field(max_length=500)


class CropPages(BaseModel):
    pages: list[int] = Field(min_length=1, max_length=2000)
    left: float = Field(ge=0, le=1)
    top: float = Field(ge=0, le=1)
    right: float = Field(ge=0, le=1)
    bottom: float = Field(ge=0, le=1)


class TextOverlay(BaseModel):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    x: float = Field(ge=0, lt=1)
    y: float = Field(ge=0, lt=1)
    text: str = Field(min_length=1, max_length=1000)
    font_size: float = Field(default=12, ge=6, le=36)


class ReplaceTextRun(BaseModel):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    span_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    expected_text: str = Field(min_length=1, max_length=2000)
    replacement: str = Field(min_length=1, max_length=2000)


class MoveTextRun(BaseModel):
    model_config = ConfigDict(extra='forbid')
    page_index: int = Field(ge=0)
    span_index: int = Field(ge=0)
    expected_revision: int = Field(ge=1)
    expected_text: str = Field(min_length=1, max_length=2000)
    vertical_offset: float = Field(ge=-1, le=1)


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
