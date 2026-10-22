# PDFium paragraph editing trial

The trial now runs inside the local Python sidecar with `pypdfium2==5.14.0`,
which packages a native PDFium library for the installed platform. The
dependency is in `backend/requirements.txt` and the PyInstaller build script
collects its runtime. A sidecar build now checks that PDFium can create a PDF
in the build environment and stops with an installation instruction if it
cannot. On another Mac or Windows machine, install the backend requirements
for that platform and build its matching sidecar there; the development
machine's installed runtime is not needed on a machine running the bundled
app. The Vue editor offers **Edit whole paragraph** after selecting a text run,
when the trial is available. This explicitly previews and replaces the detected
paragraph; normal Edit text changes only the selected run. A missing runtime or
`COURTBUNDLE_ENABLE_PDFIUM_TRIAL=0` hides the paragraph action. No external
worker, Node process, cloud service or separate document writer is required.

This is an independent Python adaptation of the PDFium object-edit approach
in [PDF Forge's MIT-licensed `PdfiumTextEditor.js`](https://github.com/tpawley2001/pdf-forge/blob/4798b301917511e4a94e1ff374e6576a360c73ad/src/pdf/PdfiumTextEditor.js):
locate the selected text object, grow adjacent same-font lines into a
paragraph, remove those objects, measure and reflow replacement text into new
PDFium objects, regenerate page content, and reject changes outside the edit
region. It does not copy PDF Forge's Electron UI, JavaScript runtime, font
loader, or full mixed-style word matching algorithm. The installed
`pypdfium2` package is dual licensed Apache-2.0/BSD-3-Clause; PDFium binary
notices still require review before distribution.

## Revision and verification path

`POST /documents/{id}/pages/paragraph-trial-preview` returns the detected
whole paragraph before editing. `POST /documents/{id}/pages/paragraph-trial`
requires both the selected run and the exact previewed paragraph plus the
expected working revision. Both use the matter lock. The edit executes on a
candidate made by `edit_working_copy()`; failed or unsupported operations
discard it without advancing revision history. There is no silent fallback to
redraw or whiteout.

The trial currently accepts an exact upright PDFium text object on an
unrotated page, a paragraph of at most 2,000 characters, and a source PDF of
at most 16 MiB. It preserves the selected object's font, size, colour and
matrix when possible. For an embedded font lacking replacement glyphs, it
tries a related standard PDF font and reports substitution. Unsupported
glyphs, object structures, collisions and layouts fail explicitly. Scanned
pixels and invisible OCR layers are not edited. The trial also rejects a
paragraph whose old or proposed area intersects an image, drawing, link,
widget or annotation.

When replacement lines need more room, the trial now moves all following
upright text objects down by the added paragraph height, preserving their
relative spacing. It checks that the moved text stays on the page and does
not cover fixed text, images, drawings, links, widgets or annotations. A
multi-column page, non-text content in the affected region or insufficient
space at the bottom still fails without publishing a revision. This is
bounded same-page text reflow, not general document layout or page insertion.
The standard single-run editor handles explicit line breaks in clear space
without PDFium.

The candidate is reopened before publication. The backend checks page count,
geometry, bookmarks, metadata, links, annotation counts, form values,
attachments, other-page text, requested extraction, extracted words and
rendered pixels outside the declared paragraph and shifted-text region. These are conservative
checks for a bounded trial, not proof of font fidelity, arbitrary PDF safety,
signature validity or secure redaction. Imported originals remain unchanged;
Undo restores the prior working revision. Python remains authoritative for
page tools, OCR, redaction and `engine.py` bundle generation.

## Current evidence and open gates

On 21 October 2026, a synthetic two-line paragraph was reflowed to four
searchable lines while a separate line stayed intact. Five focused API tests
passed: successful preview/edit/revision/original/combined export/Undo,
unfittable rejection, expansion that retains the old words, rejection of
an overlapping link without a new revision, and preservation of identical
text elsewhere on the page. The Vue production build and
Rust check passed. A frozen Apple Silicon sidecar started and returned
`available: true` from the authenticated trial endpoint; the local macOS arm64
app bundle also built. A synthetic Chrome browser journey selected the trial,
edited a paragraph and found the replacement through matter search. That run
exposed and fixed a preview race that could replace typed text with the old
paragraph; the trial input now waits for the whole-paragraph preview and
discards responses from canceled or superseded selections. Stale page overlays
are not interactive while PDF.js renders a new page. This is
a narrow engineering result; native UI review,
macOS Intel/Windows packaging, repeated save/export round trips, subset fonts,
ligatures, Hindi, forms and long documents remain to be checked on synthetic
fixtures. PDF-07 stays open.
