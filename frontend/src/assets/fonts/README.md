# Local UI font

Unmodified **Inter 4.1 variable upright**, weights 100–900, bundled by Vite.
The application uses this font for UI only; PDF.js content and document-authored
fonts/marks remain independent. No runtime font CDN or network request is needed.

Upstream release files:
- https://github.com/rsms/inter/blob/v4.1/docs/font-files/InterVariable.woff2
- https://github.com/rsms/inter/blob/v4.1/LICENSE.txt (copied verbatim as `frontend/public/licenses/Inter-OFL.txt`)

Copyright © 2016 The Inter Project Authors. SIL Open Font License 1.1;
see [the bundled full license](../../../public/licenses/Inter-OFL.txt).
Vite copies it to `dist/licenses/Inter-OFL.txt` for distribution with the app. Retrieved 27 October 2026.

SHA-256:
- InterVariable.woff2: `693b77d4f32ee9b8bfc995589b5fad5e99adf2832738661f5402f9978429a8e3`
- Inter-OFL.txt: `262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a`

The existing unused Manrope package is retained to avoid unrelated dependency
manifest/lockfile churn. It is no longer imported or included in the app bundle.
