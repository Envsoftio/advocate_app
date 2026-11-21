# MatterDraft Studio — Device licensing and pricing PRD

**Status:** Commercial direction agreed; implementation and public launch pending  
**Decision date:** 15 November 2026  
**Updated:** 17 November 2026 — seven-day trial selected; security requirements below remain to be implemented and verified  
**Scope:** Initial India offering for MatterDraft Studio

**Related specification:** [Customer and administration backend PRD](PRD_CUSTOMER_AND_ADMIN_BACKEND.md). The later connectivity decision requires internet for sign-in/initial activation and excludes permanently offline activation/manual licence files from v1; this supersedes the earlier manual-refresh proposal below. Numeric matter/page aggregate reporting is now in the backend scope; document pages/content remain excluded. Licensing enforcement details remain a separate design discussion, not completed implementation.

## 1. Decision

MatterDraft will use **device licences**. The minimum paid plan is **₹499 per device per month**. One active device requires one licence; another concurrently active device requires another licence. The price is a proposed launch price, not an existing charge or a validated willingness-to-pay finding. Tax treatment and billing terms still need to be set before publication.

The first plan should allow **unlimited matters, imported documents, pages, OCR runs and exports**, subject only to measured technical capacity and acceptable-use safeguards. Do not advertise a numerical matter or page capacity until it has passed supported-device testing. Core document preparation, review and bundle export belong in the base plan.

## 2. User and problem

The initial buyer is an Indian advocate or small chamber preparing case document bundles on a desktop computer. Usage may be intermittent. A predictable price and no document or export counting reduce hesitation to import complete matter files and review an output more than once.

## 3. Licence experience requirements

1. A paid licence activates one device at a time. Define a stable, privacy-conscious device identity and a recovery path for operating-system reinstall or hardware replacement before implementation.
2. The customer can see the active device, deactivate it and transfer the licence without contacting support in the normal case. Specify a fallback when the old device is lost or unavailable.
3. Core local processing remains usable during a documented offline grace period. An expired or temporarily unverifiable licence must not make existing local matter files inaccessible; define which creation and export actions are gated.
4. Show price, renewal cadence, device count and any tax before payment. Explain what happens on cancellation or expiry before asking the customer to subscribe.
5. Licence checks must not upload matter names, source files, OCR text or exported PDFs. Document the minimum activation data and retention policy.
6. Keep licensing separate from the deterministic PDF page-map engine and local document processing.

These are product requirements for a low-friction device licence, **not claims that licensing or billing already exists**.

### Offline verification proposal

At activation, the licensing service issues a **signed, device-bound entitlement** containing the licence ID, device ID, paid-through date and a short verification deadline. The app verifies the signature locally, so ordinary work does not require a live connection. When connected, it refreshes the entitlement and checks whether the device has been deactivated or the subscription has ended. A reasonable starting policy for testing is a **30-day verification window plus 7 days of connectivity grace**; this period is not yet an approved customer term. Show the next required check date in the app and warn before it expires. Provide a signed licence-file refresh route for customers whose computers have no internet access.

A device that never reconnects cannot report current use, receive revocation promptly or prove it is the only active copy. A device-bound token, local clock checks and a verification deadline reduce casual sharing but cannot prevent deliberate tampering. The commercial policy must accept that limit; do not claim perfect one-device enforcement. On expiry of the verification window and grace, keep existing matters readable and define the exact create/export restrictions before implementation.

**Activity measurement is optional and separate from entitlement checks.** While offline, the service has no activity data. If product analytics are approved, collect only consented aggregate counts on-device and upload them on a later connection, with no matter identifiers, titles, file names, pages or document text. If users stay offline or decline analytics, activity remains unknown; subscription payment and activation status are the available commercial signals.

## 4. Explicitly uncommitted options

The selected acquisition offer is now a **seven-day free trial**, replacing the discussed 14-day proposal. One free matter every month is not selected. The trial specification and implementation requirements are in section 7.

The discussion considered a ₹4,999 annual Solo price, a ₹1,499 monthly chamber plan, a ₹199 per-matter purchase, a free pilot and a two-device allowance. None was selected in the final decision. Do not place these options in checkout or public pricing copy without a separate decision.

The earlier suggestion of 500 documents or 10,000 pages per matter was a performance-test target, not a verified capacity or customer entitlement. The adopted commercial plan has no document-count quota.

## 5. Release and validation gates

- Complete the outstanding clean-device, Windows x64, Mac Intel, native export/save and installer checks recorded in `ACCEPTANCE_TESTS.md` before treating this as a production filing product.
- Resolve distribution licensing, including PyMuPDF, before proprietary paid distribution.
- Test activation, transfer, offline grace, renewal, cancellation, reinstall and lost-device recovery using synthetic matters on supported devices.
- Pilot the ₹499 offer with advocates who prepare bundles at different frequencies. Record conversion, active-device transfers, support contacts and reasons for declining; revise price or terms from evidence.
- Update the marketing project's pricing FAQ only when commercial terms and release readiness are approved. The earlier deferred-pricing instruction in `MatterDraft_Project_Context_and_Handoff.md` describes its prior landing-page revision; this PRD records the later decision but does not publish a price.

## 6. Acceptance criteria for a future implementation ticket

- A customer can purchase or activate one device licence and see its status and renewal terms.
- A second active device requires another licence or an explicit transfer; normal transfer is self-service.
- A temporary connectivity failure does not block work within the defined offline grace period.
- Existing local matters remain readable after expiry or cancellation, with export restrictions clearly disclosed and tested.
- Licence telemetry contains no matter content; synthetic-device and recovery tests pass on each supported platform.
- Pricing copy matches the actual checkout amount, taxes, renewal terms and entitlements.

## 7. Seven-day trial and licensing security requirements

### Decision and proposed operating policy

The owner selected a seven-day trial and requested strong protection across licence operations. The security target is **tamper resistance**, not tamper-proof offline enforcement. An attacker controlling the computer can modify the executable, restore local snapshots or suppress telemetry. Offline revocation and perfect identification of repeat users are not achievable guarantees.

Implementation defaults proposed for the selected trial:

- Start at the first successful server-side activation, with a server-recorded UTC end exactly 168 hours later. A retried activation returns the original grant and expiry; reinstall, transfer and refresh never restart the trial.
- Require a verified account and online activation; allow one trial grant per account and one active trial device. Rate-limit grant and recovery operations. Multiple accounts remain an abuse limitation; do not introduce invasive hardware fingerprinting to claim otherwise.
- Offer core preparation and export features without a payment method or artificial page quota. These are proposed trial defaults, not published customer terms.
- Allow offline work within the signed trial validity. Trial expiry receives no automatic paid-plan connectivity grace. Display the exact local expiry time and warnings in advance.
- Preserve reading, backups and retrieval of previously generated files after expiry. Define the final action-by-action editing, creation and new-export policy before implementation; never destroy or encrypt customer work as an enforcement measure.
- Separate login-session expiry, entitlement expiry, subscription cancellation and connectivity failure. An expired login session alone must not invalidate a still-valid offline entitlement.

The paid 30-day verification plus seven-day connectivity-grace proposal in section 3 remains unapproved. Before release, define paid-through, refresh-by and hard-expiry semantics independently, including payment failure and outage handling. Connectivity grace must not silently extend a trial or imply a paid renewal.

### Required security controls

1. **Asymmetric signatures:** use a maintained cryptographic library and a versioned signed entitlement format. Keep private signing keys server-side in restricted key management; ship only trusted public verification keys. Specify an algorithm allowlist, issuer, audience, token type, device-key binding, entitlement ID, issue time, validity boundaries and key ID. Reject malformed, unsigned, wrong-audience, wrong-device and unsupported-algorithm tokens. Never load a verification key from a token-supplied arbitrary URL.
2. **Key lifecycle:** document signing-key rotation, overlapping verification keys, compromise response and emergency recovery. Offline clients cannot learn about compromised or revoked keys immediately; bound and disclose that exposure through validity deadlines.
3. **Device proof:** generate an installation key pair and protect its private key with OS credential storage, preferably non-exportable where supported. Verify fresh server challenges when registering or refreshing a device; enforce nonce expiry and single use. Test supported-platform behavior rather than assuming hardware-backed storage exists everywhere. Do not distribute device private keys in matter backups.
4. **Authentication:** use the external browser with Authorization Code + PKCE, validated redirect destinations and state. Do not embed an OAuth client secret in the desktop app. Use short-lived access tokens and rotated or sender-constrained refresh tokens, with replay detection and recovery behavior. Keep credentials out of URLs, logs and browser-accessible persistent storage where avoidable.
5. **Server authority:** authorize every operation against account ownership and roles. Allocate seats, start trials and transfer devices with atomic database changes, uniqueness constraints and idempotency keys. Concurrent requests must not allocate excess seats or create extra trials. Entitlement issuance must reflect committed current state and reject stale device generations.
6. **Local enforcement:** check entitlements at protected operation entry points in the native/local service, not only Vue controls. Keep the PDF page-map engine independent. Check before starting work and specify safe completion of in-flight operations; expiry must not corrupt files or discard completed work. Verify signatures using trusted code, not a mutable SQLite `licensed` flag.
7. **Time and rollback:** use server timestamps, a protected last-seen-time record and monotonic elapsed time where applicable. Suspicious rollback requests online recovery; it never deletes matters. Test sleep, reboot, timezone changes and incorrect clocks. Local snapshots and executable patches remain limitations.
8. **Billing:** verify webhook authenticity over the provider-required original payload, process duplicates idempotently and tolerate out-of-order delivery. Do not grant paid access from a browser checkout-success redirect. Reconcile authoritative provider state, persist retry work and audit renewals, refunds, cancellations and chargebacks. Specify access policy for each state before release.
9. **Transfer and recovery:** require authenticated ownership and reauthentication for sensitive changes. Atomically deactivate the prior assignment, advance its generation and issue the new assignment. Disclose that an offline old device can retain access until its existing entitlement expires. Provide lost-device/reinstall recovery, abuse limits and audited support overrides without restarting a trial.
10. **Operations:** enforce TLS, request validation, rate limits, least-privilege service credentials, admin MFA and role checks. Protect cookie-based portal mutations against CSRF. Keep production secrets out of preview deployments and client bundles. Maintain audit records without raw tokens, private keys or document data; test database restoration and signing-key recovery separately.

Application/update signing and entitlement signing are separate controls. Neither justifies claims of an uncrackable application. Obfuscation may be an additional deterrent, never the root of trust.

### Required verification before claiming completion

- Seven-day boundary, activation retries, simultaneous trial requests, reinstall, transfer and upgrade preserve the correct original expiry.
- Forged signatures, edited claims, algorithm substitution, wrong issuer/audience/device, replayed challenges and unknown signing keys are rejected.
- Concurrent activations/transfers and stale refreshes cannot resurrect a revoked assignment or exceed allocated seats.
- Refresh-token replay, lost-device recovery and account ownership violations are handled without exposing another customer's licence.
- Duplicate, delayed, reordered and forged payment events cannot grant incorrect access; interrupted processing recovers through reconciliation.
- Offline startup, service outages, clock rollback, sleep/reboot, key rotation and paid/trial expiry follow the disclosed policy.
- Expiry during work preserves originals and completed artifacts; reading, backup and permitted retrieval continue.
- Synthetic native tests on each supported OS exercise protected operations through the local API as well as the UI. Security review and dependency checks precede paid launch.

No licensing code, deployment or security verification is claimed by this PRD update.

### Standards references

- [RFC 8252 — OAuth 2.0 for Native Apps](https://www.rfc-editor.org/info/rfc8252/): external-browser native authentication and PKCE.
- [RFC 9700 — OAuth 2.0 Security Best Current Practice](https://www.rfc-editor.org/rfc/rfc9700.html): token replay protection and refresh-token handling.
- [RFC 8725 — JSON Web Token Best Current Practices](https://www.rfc-editor.org/rfc/rfc8725.html): applicable if JWT/JWS is selected; explicit algorithms, issuer/audience validation and token-type separation.
