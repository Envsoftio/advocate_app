# B04B implementation — reviewed insertion policy and supplemental generation

Date: 29 October 2026

[Certain] B04B is implemented for the available Apple Silicon development Mac. It does not define a court-approved numbering rule and does not qualify Windows, Intel Mac, a clean device, or a minimum macOS release.

## Requirement-to-evidence checklist

| Requirement | Delivered evidence |
|---|---|
| Explicit reviewed policies | A plan must select and accept either `separate_supplement` or `preserve_printed_labels`. The accepted mode and a digest of the exact plan contract are persisted; any title, ordered page-ID, settings, snapshot, baseline, or policy change invalidates readiness. |
| Conservative reconciliation | Preservation is allowed only when removing the selected inserted pages exactly restores each baseline source sequence by rendered/text fingerprint. New-document inserts, before-first inserts, replacements, deletions, reorderings, missing baseline evidence, stale identities, unsupported labels, and collisions block acceptance. Fingerprints prove a pure insertion; they do not create cross-revision stable identity. |
| Real effect preview | The BU09 screen shows the immutable baseline, exact selected IDs in expandable evidence, earlier/inserted source positions, printed/viewer labels, combined positions, affected volume boundaries, and explicit index/bookmark/link/viewer-label effects. It states that physical positions change. |
| Deterministic output | `backend/app/engine.py` resolves the exact selected IDs, creates the shared map, applies reviewed labels to that same map, and derives PDF index, CSV ranges, bookmarks, viewer labels, volume numbering, combined offsets, validation contract, and manifest from it. The eight-page fixture produces ten physical pages only because its saved settings generate two front-matter pages. |
| Distinct immutable artifact | The supplemental job has kind `supplement-build`; its manifest records plan ID/version, baseline, policy, exact selection, revisions, settings, policy preview and artifact kind. It publishes to a new build directory and never overwrites the baseline. Storage staleness is plan-contract aware. |
| Connected workflow | Membership and numbering Apply/Cancel, page-map preview, policy review, build progress/failure/retry, output checks, generated-PDF review, export, Storage and saved-plan reopen remain connected. Primary membership/settings/counts are not changed by a supplement. |
| Failure and restart handling | Empty plans, stale or missing IDs, concurrent plan versions, delayed previews, changed settings, service/save failures, interrupted supplemental jobs and failed builds fail closed. Maps/readiness are invalidated. The native QA app generated, reviewed and saved the artifact, then cleanly quit/relaunched with the plan and artifact present. |
| Preservation | Backend tests hash imported originals and every prior baseline file before/after generation. Native Save produced a copy whose SHA-256 exactly matched the saved artifact. |

## Policy and persistence decisions

- `separate_supplement` is the supported deliberate-renumbering path. It uses the plan's supplemental numbering and rebuilds navigation inside a distinct artifact.
- `preserve_printed_labels` currently supports only decimal suffixes after an existing numeric label, such as `3.1` through `3.8`. The notation is a validated technical capability, not a universal rule. Existing labels remain unchanged; later physical/combined positions move.
- Preservation requires selected pages inside a baseline-recorded document. Arbitrary new-document supplements remain supported through `separate_supplement`.
- A policy acceptance stores `contract_digest` and `preview_digest` on the plan version. Build submission and the queued task both recheck plan version, contract, input snapshot and policy.
- Supplemental manifests and output checks remain separate from the primary build. Portable backups still do not include supplemental plans, the source-page identity registry or prior output directories; that B04A limitation is unchanged.

## Changed paths

- Backend: `app/engine.py`, `app/supplements.py`, `app/main.py`, `app/schemas.py`, `app/storage.py`, and `tests/test_supplemental_plans.py`.
- Frontend: `SupplementWorkspace.vue`, `BuildWorkspace.vue`, `App.vue`, `supplement-design.css`, `lib/types.ts`, and `e2e/supplement-plans.spec.ts`.
- Documentation: this file plus `README.md`, `IMPLEMENTATION_TASKS.md`, `PROJECT_CONTEXT_AND_HANDOFF.md`, and `ACCEPTANCE_TESTS.md`.
- No schema migration was needed beyond B04A schema 20.

## BU09 visual verification

The supplied `CourtBundle_JSON_For_Codex/implementation/previews/BU09.png` was inspected before implementation. The relevant JSON nodes, descendant overrides, interaction metadata and resolved light/dark tokens were parsed directly; no replacement concept or Pencil/MCP output was used. The UI-taste image-to-UI, new-work, operate and craft guidance informed the comparison without overriding MatterDraft identity.

At 1440×900 and 100%, the running MatterDraft screen measures:

- status strip: `x=208, y=214, w=1208, h=42`;
- insertion card: `x=208, y=278, w=1208, h=420`;
- warning/action row: `x=208, y=718, w=1208` with a 984/16/208 split.

Runtime evidence is under `docs/ui-evidence/b04b/`: `BU09-1440-light-100.png`, eight 1024×768 light/dark 90/100/110/125% captures, `BU09-side-by-side.png`, and `BU09-detail-crop.png`. The adaptation at compact widths keeps the table horizontally scrollable and stacks the warning/action controls instead of clipping them. Exact IDs and reconciliation details move below the reference viewport in a disclosure so the approved composition remains intact while evidence stays available.

## Verification record

| Command or journey | Result |
|---|---|
| `cd backend && .venv312/bin/python -m pytest -q` | **325 passed**, six dependency deprecation warnings. |
| `cd frontend && npm run build` | **Pass**; existing large-chunk warning remains. |
| `cd frontend && npm run test` | **44 passed**. |
| Final focused regression and B04 browser run | **8 passed**, 47.9s: reconciled BU09 geometry/matrix, accepted preservation, generated a ten-page artifact, ran output checks and opened the generated PDF read-only, while retaining representative reader/navigation behavior. |
| Final complete browser suite | **120 passed, 1 unrelated thumbnail-focus timing failure, 1 opt-in vault skip**, 11.0m. B04B passed. The exact unchanged 10/100/1000-page thumbnail-rail spec then passed 1/1 in 9.3s. |
| `.venv312/bin/python scripts/build_sidecar.py` | **Pass**; PyInstaller 6.22.3 / Python 3.12.13, arm64 sidecar SHA-256 `dc1f0e8d550b9d4f0d0666c75d4d125ddb926cfb8024029bbc8db957d5a11854`. |
| `npm run tauri:build -- --bundles app` | **Pass**; unsigned 70.14 MiB Apple Silicon app. Final executable SHA-256 `1ac45993d698209fc995377ed7cd35df970a1dd7a5317a8fe2c3746fd1fce1ca`; embedded sidecar hash matches. |

## Native findings

A uniquely identified debug QA app used the freshly built frozen sidecar on `127.0.0.1:8789` and `/private/tmp/matterdraft-b04b-native-store.htfavZ`. It did not open the installed app or the user's regular data. With synthetic `samples/01_petition.pdf`, it showed the real BU09 map, accepted the reviewed policy, generated build `e9cfb436…`, displayed the ten-page PDF read-only, and used the macOS Save panel to write `combined.pdf` under `/private/tmp/matterdraft-b04b-native-save`.

The native-saved file was 78,663 bytes with SHA-256 `d1a1aeea20d68aeb529064d2decf49fa5a3a73616d5f5acebda86973f506e0bd`, exactly matching the artifact manifest. After Quit and relaunch of a clean QA identity, the 11-page working revision, reviewed plan, supplemental artifact, review/export controls and exact build remained available.

An older QA identity retained inconsistent WebView state and showed the welcome pane beside a restored matter shell. It was not counted as a restart pass. Browser views keep their proven named `out-in` animation, while native WebView now disables the outer transition name and mode. The final freshly rebuilt QA app opened the 11-page working PDF, exposed build `e9cfb436…`, quit, relaunched, rendered the PDF again and showed the persisted reviewed plan, 10-page artifact, output checks and review/export controls. The installed MatterDraft application was not replaced; the isolated QA app and sidecar were stopped.

## Remaining limitations and next bounded task

- Windows x64, Intel Mac, clean-device/minimum-macOS, production encrypted-vault startup, low-disk/interrupted native Save, assistive-technology and native Office/print checks remain open.
- Preservation does not support insertion before the first baseline page, arbitrary replacements/deletions, a new document interleaved into an old document, or nonnumeric/suffix-colliding labels. The UI blocks these cases and offers a separate supplement.
- Fingerprint reconciliation renders each involved baseline/current page and is intentionally conservative; large-matter performance limits are not established here.
- No legal approval, filing acceptance, verified OCR, or verified signature is inferred.

Next: **B04C audience variants only**—derive independently reviewed audience-specific artifacts from an explicit saved source contract. Do not broaden B04C into new insertion notation or claim whole-B04 legal approval.
