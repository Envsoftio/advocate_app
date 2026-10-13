import { rmSync } from 'node:fs'

export default function teardown() {
  const dataDir = process.env.COURTBUNDLE_E2E_DATA_DIR
  if (dataDir?.includes('courtbundle-e2e-')) rmSync(dataDir, { recursive: true, force: true })
}
