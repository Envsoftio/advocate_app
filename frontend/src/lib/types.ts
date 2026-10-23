export interface Matter {
  id: string
  name: string
  court_profile: string
  case_type: string
  petitioner: string
  respondent: string
  advocate: string
  case_number: string
  max_pages_per_volume: number
  keep_groups_intact: boolean
  created_at: string
  updated_at: string
  archived_at?: string | null
}
export interface MatterNew extends Omit<Matter, 'id' | 'created_at' | 'updated_at'> {}
export type DocKind = 'petition' | 'affidavit' | 'order' | 'annexure' | 'application' | 'evidence' | 'other'
export type SignatureStatus = 'certificate_signature_detected' | 'no_certificate_signature_detected' |
  'inspection_inconclusive' | 'visible_signature_only' | 'unsigned'
export interface MatterDocument {
  id: string
  matter_id: string
  title: string
  kind: DocKind
  part: number
  group_key: string
  group_role: 'member' | 'parent'
  label: string
  label_status: 'unreviewed' | 'confirmed'
  label_source: 'none' | 'legacy' | 'user' | 'proposal' | 'filename'
  document_date: string
  date_status: 'unknown' | 'ambiguous' | 'suggested' | 'confirmed'
  date_source: 'none' | 'user' | 'extracted' | 'filename' | 'filesystem'
  date_note: string
  source_file_mtime: string
  position: number
  page_count: number
  ocr_status: string
  ocr_provenance_json: string
  original_name: string
  source_sha256: string
  certificate_signature_status: 'detected' | 'not_detected' | 'unknown'
  certificate_signature_count: number
  signature_inspection_note: string
  visible_signature_status: 'unreviewed' | 'present' | 'absent'
  signature_status: SignatureStatus
  revision: number
  created_at: string
  updated_at: string
  archived_at?: string | null
}
export interface ManualBookmark {
  id: string
  title: string
  document_id: string
  page_index: number
  target_revision: number
  parent_id: string | null
  position: number
  resolved: boolean
}
export interface ReviewedLink {
  id: string
  title: string
  source_document_id: string
  source_page_index: number
  source_revision: number
  rect_x: number
  rect_y: number
  rect_width: number
  rect_height: number
  kind: 'internal' | 'external'
  target_document_id: string | null
  target_page_index: number | null
  target_revision: number | null
  uri: string
  position: number
  resolved: boolean
}
export interface PageAnnotation {
  id: string
  document_id: string
  page_index: number
  target_revision: number
  kind: 'image' | 'highlight' | 'comment' | 'draw' | 'box' | 'ellipse' | 'line' | 'arrow' | 'check' | 'cross' | 'date' | 'text' | 'redaction'
  rect_x: number
  rect_y: number
  rect_width: number
  rect_height: number
  body: string
  color: 'yellow' | 'green' | 'blue' | 'pink'
  asset_relpath: string | null
  asset_mime: string | null
  output_scope: 'private' | 'pdf'
  position: number
  resolved: boolean
}
export type MarkPosition = 'top_left' | 'top_center' | 'top_right' | 'center' |
  'bottom_left' | 'bottom_center' | 'bottom_right'
export interface OutputMarks {
  index_style: 'compact' | 'readable'
  part_extracts_enabled: boolean
  watermark_enabled: boolean
  watermark_text: string
  watermark_position: MarkPosition
  watermark_font_size: number
  watermark_opacity: number
  timestamp_enabled: boolean
  timestamp_format: 'date' | 'datetime'
  timestamp_timezone: string
  timestamp_position: MarkPosition
  timestamp_font_size: number
  timestamp_prefix: string
  bates_enabled: boolean
  bates_prefix: string
  bates_start: number
  bates_padding: number
  bates_position: MarkPosition
  bates_font_size: number
  bates_include_index: boolean
  bates_include_separator: boolean
  bates_restart_per_volume: boolean
  annexure_mode: 'none' | 'label_stamp' | 'separator'
  annexure_stamp_scope: 'first' | 'all'
  annexure_stamp_position: MarkPosition
  annexure_stamp_font_size: number
  annexure_overrides: Record<string, 'none' | 'label_stamp' | 'separator'>
}
export interface OutputMarkPreview {
  kind: 'watermark' | 'timestamp' | 'bates' | 'annexure_label'
  text: string
  font_size: number
  position: MarkPosition
  opacity: number
  overlaps_content: boolean
  relative_rect: [number, number, number, number]
}
export interface PageSearchMatch {
  document_id: string
  title: string
  revision: number
  page: number
  snippet: string
}
export interface PageSearchResult {
  matches: PageSearchMatch[]
  truncated: boolean
}
export interface Check {
  severity: 'error' | 'warning' | 'info'
  code: string
  message: string
  document_id: string | null
}
export interface CourtProfile {
  id: string
  name: string
  description: string
  status: string
  effective_from: string | null
  last_checked: string
  max_file_mb: number | null
  require_searchable_text: boolean
  require_bookmarks: boolean
  disallow_pdf_encryption: boolean
  sources: Array<{ title: string, url: string }>
  filing_guidance?: {
    title: string
    scope: string
    status: string
    reviewed_through: string
    requirements: Array<{
      title: string
      detail: string
      source_label: string
      source_url: string
      effective_from: string | null
    }>
    templates: Array<{
      title: string
      format: string
      purpose: string
      url: string
    }>
    source_conflicts: string[]
  }
}
export interface ExportManifest {
  id: string
  matter_id: string
  created_at: string
  pages: number
  volumes: number
  stale?: boolean
  files: Array<{ name: string, bytes: number }>
  checks: Check[]
}
export interface Job {
  id: string
  matter_id: string
  kind: 'build' | 'ocr' | 'compress'
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted'
  target_id?: string | null
  retry_job_id?: string | null
  created_at?: string
  progress: number
  message: string
  error: string | null
  result: ExportManifest | Record<string, unknown> | null
}
