export interface Matter {
  id: string
  name: string
  court_forum: string
  client_reference: string
  internal_reference: string
  notes: string
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
export type MatterNew = Omit<Matter, 'id' | 'created_at' | 'updated_at' | 'court_forum' | 'client_reference' | 'internal_reference' | 'notes'> &
  Partial<Pick<Matter, 'court_forum' | 'client_reference' | 'internal_reference' | 'notes'>>
export type DocKind = 'petition' | 'affidavit' | 'order' | 'annexure' | 'application' | 'evidence' | 'other'
export type SignatureStatus = 'certificate_signature_detected' | 'no_certificate_signature_detected' |
  'inspection_inconclusive' | 'visible_signature_only' | 'unsigned'
export interface MatterDocument {
  bundle_included: number
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
  created_at: string | null
  updated_at: string | null
}
export type MarkPosition = 'top_left' | 'top_center' | 'top_right' | 'center' |
  'bottom_left' | 'bottom_center' | 'bottom_right'
export interface OutputMarks {
  volume_max_bytes?: number | null
  volume_splits?: Record<string,{revision:number;after_pages:number[]}>
  processing_quality?: 'original' | 'lossless' | 'balanced' | 'small'
  cleanup_metadata?: boolean
  print_layout?: 'screen'
  office_docx?: boolean
  office_xlsx?: boolean
  office_pptx?: boolean
  numbering_style?: 'arabic' | 'roman_lower' | 'roman_upper'
  numbering_restart?: 'volume' | 'continuous' | 'section'
  numbering_start?: number
  numbering_prefix?: string
  numbering_position?: 'bottom_left' | 'bottom_center' | 'bottom_right' | 'top_left' | 'top_right'
  numbering_front_matter?: 'physical' | 'roman' | 'none'
  numbering_pdf_labels?: boolean
  numbering_linked_index?: boolean
  numbering_back_link?: boolean
  front_matter: 'index_only' | 'cover_index'
  index_date: boolean
  cover_heading: string
  cover_reference: string
  cover_title: string
  index_style: 'compact' | 'readable' | 'review'
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
  validation_run_id?: string
  effective_input_digest?: string | null
  finding_id?: string
  input_snapshot?: string | null
  target_revision?: number
  page_index?: number
  artifact?: 'working' | 'original'
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
  artifact_kind?: 'primary' | 'supplement' | 'variant'
  variant_plan_id?: string | null
  variant_plan_version?: number | null
  variant_name?: string | null
  supplement_plan_id?: string | null
  supplement_plan_version?: number | null
  baseline_build_id?: string | null
  insertion_policy?: 'separate_supplement' | 'preserve_printed_labels' | null
  files: Array<{ name: string, bytes: number, sha256?: string }>
  input_revisions?: Array<{document_id:string;revision:number}>
  page_map_snapshot?: Array<{printed_labels?:string[];id:string;title:string;volume:number;start_page:number;end_page:number;combined_start_page:number;combined_end_page:number}>
  checks: Check[]
}
export interface Job {
  id: string
  matter_id: string
  kind: 'build' | 'supplement-build' | 'variant-build' | 'ocr' | 'compress'
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted'
  target_id?: string | null
  retry_job_id?: string | null
  created_at?: string
  progress: number
  message: string
  error: string | null
  result: ExportManifest | Record<string, unknown> | null
}

export interface BundleMembershipState {
  bundle_id: string
  title: string
  revision: number
  input_snapshot: string
  documents: Array<Pick<MatterDocument, 'id'|'title'|'part'|'group_key'|'page_count'|'position'|'revision'> & {bundle_included:number}>
  included_documents: number
  included_source_pages: number
  reference_documents: number
  reference_source_pages: number
}
