import type { ExportManifest } from './types';

/** Read the frozen engine map; do not reconstruct source offsets from current documents. */
export function generatedSourcePage(
  row: NonNullable<ExportManifest['page_map_snapshot']>[number] | undefined,
  filename: string,
  physicalPage: number,
): number | null {
  if (!row || !Number.isInteger(physicalPage)) return null;
  const combined = filename === 'combined.pdf';
  if (!combined && filename !== `volume-${String(row.volume).padStart(2, '0')}.pdf`) return null;
  const first = combined ? row.combined_start_page : row.start_page;
  const last = combined ? row.combined_end_page : row.end_page;
  if (physicalPage < first || physicalPage > last) return null;
  return (row.source_start ?? 0) + physicalPage - first + 1;
}
