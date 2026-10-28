import { rmSync } from 'node:fs'

export default function teardown() {
  const dataDir = process.env.MATTERDRAFT_E2E_DATA_DIR
  if (dataDir?.includes('matterdraft-e2e-')) rmSync(dataDir, { recursive: true, force: true })
}
