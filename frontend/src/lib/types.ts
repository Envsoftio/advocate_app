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
export interface MatterDocument {
  id: string
  matter_id: string
  title: string
  kind: DocKind
  part: number
  group_key: string
  label: string
  position: number
  page_count: number
  ocr_status: string
  original_name: string
  source_sha256: string
  revision: number
  created_at: string
  updated_at: string
  archived_at?: string | null
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
  kind: 'build' | 'ocr'
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted'
  target_id?: string | null
  retry_job_id?: string | null
  created_at?: string
  progress: number
  message: string
  error: string | null
  result: ExportManifest | Record<string, unknown> | null
}
