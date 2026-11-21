import { test, expect } from '@playwright/test';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';

// Real Rust core behind a test IPC bridge; native Tauri is verified separately.
for (const theme of ['light', 'dark']) {
  test(`Rust metadata edit undo redo archive restart restore ${theme}`, async ({ page }) => {
    const temp = await mkdtemp(resolve(tmpdir(), 'matterdraft-rust-ui-'));
    const driver = resolve(
      import.meta.dirname,
      '../../rust-backend/target/debug/examples/contract',
    );
    let queue: Promise<unknown> = Promise.resolve();
    let nextPicked: { name: string; bytes: Buffer } | undefined;
    const picked = new Map<string, { name: string; bytes: Buffer }>();
    let pickNumber = 0;
    const pdfReaders = new Map<string, number[]>();
    await page.exposeFunction('rustTestInvoke', async (command: string, payload: unknown) => {
      if (command === 'get_backend_connection') return { base_url: 'rust-ipc:', token: '' };
      if (command === 'rust_pick_import_pdf') {
        const selected = nextPicked;
        nextPicked = undefined;
        if (!selected) return { status: 200, body: { cancelled: true } };
        const hash = createHash('sha256').update(selected.bytes).digest('hex');
        if (
          selected.bytes.length !== 6004 ||
          hash !== '76da5e34c1799a5aff3d3e9bec8cc689cb6cf8430dfc163f93bfc3ec286bf17a'
        )
          return {
            status: 422,
            body: {
              name: selected.name,
              detail: 'Only the exact samples/01_petition.pdf synthetic fixture is allowed',
            },
          };
        const token = `pick-${++pickNumber}`;
        picked.set(token, selected);
        return {
          status: 200,
          body: { token, name: selected.name, hash, size: selected.bytes.length },
        };
      }
      if (command === 'rust_discard_picked_pdf') {
        const token = (payload as { token: string }).token;
        return { status: 200, body: { discarded: picked.delete(token) } };
      }
      if (command === 'rust_read_pdf') {
        const { token, begin, end } = payload as { token: string; begin: number; end: number };
        const bytes = pdfReaders.get(token);
        if (!bytes) throw new Error('Expired test PDF reader');
        return bytes.slice(begin, end);
      }
      if (command === 'rust_close_pdf') {
        pdfReaders.delete((payload as { token: string }).token);
        return null;
      }
      if (command === 'save_working_pdf') return true; // Native publishing is covered by Tauri.
      if (!['rust_request', 'rust_import_picked_pdf', 'rust_open_pdf'].includes(command))
        throw new Error(`Unsupported test command ${command}`);
      const result = queue.then<{
        status: number;
        body: { length: number; bytes: number[] };
      }>(
        () =>
          new Promise((resolveReply, reject) => {
            const child = execFile(driver, [resolve(temp, 'store')], (error, stdout) => {
              if (error) reject(error);
              else {
                try {
                  resolveReply(JSON.parse(stdout));
                } catch (error) {
                  reject(error);
                }
              }
            });
            const input =
              command === 'rust_import_picked_pdf'
                ? (() => {
                    const { matterId, token } = payload as { matterId: string; token: string };
                    const selected = picked.get(token);
                    if (!selected) throw new Error('Expired test picker token');
                    picked.delete(token);
                    return {
                      method: 'IMPORT',
                      path: `/matters/${matterId}/documents/raw`,
                      body: { filename: selected.name, bytes: Array.from(selected.bytes) },
                    };
                  })()
                : command === 'rust_open_pdf'
                  ? {
                      method: 'SNAPSHOT',
                      path: `/documents/${(payload as { documentId: string }).documentId}`,
                      body: (payload as { source: unknown }).source,
                    }
                  : payload;
            child.stdin!.end(JSON.stringify(input) + '\n');
          }),
      );
      queue = result.catch(() => undefined);
      const reply = await result;
      if (command !== 'rust_open_pdf' || reply.status !== 200) return reply;
      const token = `pdf-${++pickNumber}`;
      pdfReaders.set(token, reply.body.bytes);
      return { status: 200, body: { token, length: reply.body.length } };
    });
    await page.addInitScript((theme) => {
      localStorage.setItem('courtbundle.appearance.v1', JSON.stringify({ theme, scale: 100 }));
      Object.assign(window, {
        __TAURI_INTERNALS__: {
          invoke: async (command: string, payload: unknown) => {
            const value = await (
              window as unknown as { rustTestInvoke: (c: string, p: unknown) => Promise<unknown> }
            ).rustTestInvoke(command, payload);
            return command === 'rust_read_pdf' ? Uint8Array.from(value as number[]).buffer : value;
          },
        },
      });
    }, theme);
    try {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto('/');
      await page.getByRole('button', { name: 'New matter', exact: true }).first().click();
      await page
        .getByRole('textbox', { name: 'Matter name', exact: true })
        .fill('Synthetic Rust browser');
      await page.getByRole('button', { name: 'Create matter', exact: true }).click();
      await expect(
        page.getByRole('heading', { name: 'Synthetic Rust browser', exact: true }),
      ).toBeVisible();
      await expect(
        page.getByText(/Only the exact samples\/01_petition.pdf synthetic fixture is eligible/),
      ).toBeVisible();
      await page.getByRole('button', { name: 'Matter details', exact: true }).click();
      await page.getByRole('button', { name: 'Edit matter', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: 'Edit matter', exact: true });
      const name = dialog.getByRole('textbox', { name: 'Matter name', exact: true });
      await expect(name).toBeFocused();
      await name.fill('Discarded draft');
      await page.keyboard.press('Escape');
      await expect(dialog).toHaveCount(0);
      await expect(
        page.getByRole('heading', { name: 'Synthetic Rust browser', exact: true }),
      ).toBeVisible();
      await page.getByRole('button', { name: 'Matter details', exact: true }).click();
      await page.getByRole('button', { name: 'Edit matter', exact: true }).click();
      await expect(name).toHaveValue('Synthetic Rust browser');
      await name.fill('Synthetic Rust edited');
      await dialog
        .getByRole('textbox', { name: 'Notes', exact: true })
        .fill('Synthetic durable note');
      await expect(page.locator('.modal-enter-active,.modal-leave-active')).toHaveCount(0);
      await page.screenshot({
        animations: 'disabled',
        path: `/private/tmp/rust-matter-edit-${theme}.png`,
      });
      await dialog.getByRole('button', { name: 'Save changes' }).click();
      await expect(
        page.getByRole('heading', { name: 'Synthetic Rust edited', exact: true }),
      ).toBeVisible();
      const undo = page.getByRole('button', { name: 'Undo last matter change', exact: true });
      const redo = page.getByRole('button', { name: 'Redo last matter change', exact: true });
      await expect(undo).toBeEnabled();
      await expect(undo).toHaveAttribute('title', /Matter settings/);
      await undo.click();
      await expect(
        page.getByRole('heading', { name: 'Synthetic Rust browser', exact: true }),
      ).toBeVisible();
      await expect(undo).toBeDisabled();
      await expect(redo).toBeEnabled();
      await redo.click();
      await expect(
        page.getByRole('heading', { name: 'Synthetic Rust edited', exact: true }),
      ).toBeVisible();
      await expect(redo).toBeDisabled();
      await page.getByRole('button', { name: 'Matter details', exact: true }).click();
      page.once('dialog', (dialog) => dialog.accept());
      await page.getByRole('button', { name: 'Archive matter', exact: true }).click();
      await expect(
        page.getByRole('button', { name: 'Open Synthetic Rust edited', exact: true }),
      ).toHaveCount(0);
      await page.reload();
      await page.getByRole('combobox', { name: 'Filter matters', exact: true }).click();
      await page.getByRole('option', { name: 'Archived matters', exact: true }).click();
      await expect(
        page.getByRole('button', { name: 'Restore Synthetic Rust edited', exact: true }).first(),
      ).toBeVisible();
      await page.screenshot({
        animations: 'disabled',
        path: `/private/tmp/rust-matter-archive-${theme}.png`,
      });
      await page
        .getByRole('button', { name: 'Restore Synthetic Rust edited', exact: true })
        .first()
        .click();
      await expect(
        page.getByRole('heading', { name: 'Synthetic Rust edited', exact: true }),
      ).toBeVisible();
      await expect(undo).toBeEnabled();
      await expect(redo).toBeDisabled();
      await page.screenshot({
        animations: 'disabled',
        path: `/private/tmp/rust-matter-restored-${theme}.png`,
      });
      await page.reload();
      await page
        .getByRole('button', { name: 'Open Synthetic Rust edited', exact: true })
        .first()
        .click();
      await page.getByRole('button', { name: 'Matter details', exact: true }).click();
      await expect(page.getByText('Synthetic durable note', { exact: true })).toBeVisible();
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: 'Import documents', exact: true }).last().click();
      await expect(page.getByText(/Exact samples\/01_petition.pdf fixture only/)).toBeVisible();
      const samplePath = resolve(import.meta.dirname, '../../samples/01_petition.pdf');
      nextPicked = {
        name: 'changed.pdf',
        bytes: Buffer.concat([await readFile(samplePath), Buffer.from('\n/ByteRange [0 1 2 3]')]),
      };
      await page.getByRole('button', { name: 'Choose files' }).click();
      await expect(
        page.getByText('Only the exact samples/01_petition.pdf synthetic fixture is allowed'),
      ).toBeVisible();
      await expect(page.getByRole('button', { name: 'Import 0 files' })).toBeDisabled();
      for (const fixture of ['escaped_name_certificate.pdf', 'encrypted.pdf', 'unsigned.pdf']) {
        nextPicked = {
          name: fixture,
          bytes: await readFile(
            resolve(import.meta.dirname, '../../samples/rust_pdf_corpus', fixture),
          ),
        };
        await page.getByRole('button', { name: 'Choose files' }).click();
        await expect(
          page
            .getByText('Only the exact samples/01_petition.pdf synthetic fixture is allowed')
            .last(),
        ).toBeVisible();
        await expect(page.getByRole('button', { name: 'Import 0 files' })).toBeDisabled();
      }
      nextPicked = { name: '01_petition.pdf', bytes: await readFile(samplePath) };
      await page.getByRole('button', { name: 'Choose files' }).click();
      await expect(page.getByRole('button', { name: 'Import 1 file' })).toBeEnabled();
      await page.getByRole('button', { name: 'Import 1 file' }).click();
      await expect(page.getByText('1 of 1 files imported')).toBeVisible();
      await page.getByRole('button', { name: 'View imported documents' }).click();
      await expect(page.getByRole('button', { name: /Select 01_petition/ })).toBeVisible();
      const preview = page.getByRole('region', { name: 'Working PDF preview' });
      await expect(preview.locator('canvas').first()).toBeVisible();
      await expect(preview.getByText('Page 1 of 3')).toBeVisible();
      await preview.getByRole('button', { name: 'Next page' }).focus();
      await page.keyboard.press('Enter');
      await expect(preview.getByText('Page 2 of 3')).toBeVisible();
      const paintedPage = preview.locator('canvas').first();
      await expect(paintedPage).toBeVisible();
      await expect
        .poll(() =>
          paintedPage.evaluate((canvas: HTMLCanvasElement) => {
            const pixels = canvas
              .getContext('2d')
              ?.getImageData(0, 0, canvas.width, canvas.height).data;
            if (!pixels) return false;
            for (let index = 0; index < pixels.length; index += 4) {
              if (pixels[index + 3] === 255 && pixels[index] < 50 && pixels[index + 1] < 50)
                return true;
            }
            return false;
          }),
        )
        .toBe(true);
      await preview.getByRole('button', { name: 'Save copy' }).click();
      await expect(page.getByText('Working PDF saved.')).toBeVisible();
      const listBounds = await page.locator('.pilot-documents .docs-panel').boundingBox();
      const previewBounds = await page.locator('.pilot-documents .preview-panel').boundingBox();
      expect(
        listBounds && previewBounds && previewBounds.x >= listBounds.x + listBounds.width,
      ).toBeTruthy();
      await page.screenshot({
        animations: 'disabled',
        path: `/private/tmp/rust-import-${theme}.png`,
      });
      page.once('dialog', (dialog) => dialog.accept());
      await page.getByRole('button', { name: 'Archive document' }).click();
      await expect(page.getByRole('button', { name: /Select 01_petition/ })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Restore', exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Restore', exact: true }).click();
      await expect(page.getByRole('button', { name: /Select 01_petition/ })).toBeVisible();
      await page
        .locator('#main-content')
        .getByRole('button', { name: 'Undo last matter change' })
        .focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('button', { name: 'Restore', exact: true })).toBeVisible();
      await page
        .locator('#main-content')
        .getByRole('button', { name: 'Redo last matter change' })
        .focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('button', { name: /Select 01_petition/ })).toBeVisible();
      await page.screenshot({
        animations: 'disabled',
        path: `/private/tmp/rust-lifecycle-${theme}.png`,
      });
    } finally {
      await rm(temp, { recursive: true, force: true });
    }
  });
}
