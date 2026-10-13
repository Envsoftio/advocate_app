export function prettyDate(input: string): string {
  return new Date(input).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })
}
export function bytesLabel(input: number): string {
  if (input < 1024) return `${input} B`
  if (input < 1048576) return `${(input / 1024).toFixed(1)} KB`
  return `${(input / 1048576).toFixed(2)} MB`
}
export function friendlyError(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}
