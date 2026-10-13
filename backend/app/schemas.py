from typing import Literal
from pydantic import BaseModel, Field, ConfigDict

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


class ReorderDocuments(BaseModel):
    ids: list[str]


class ReorderPages(BaseModel):
    order: list[int]


class RotatePage(BaseModel):
    page_index: int = Field(ge=0)
    degrees: int


class DeletePage(BaseModel):
    page_index: int = Field(ge=0)


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
