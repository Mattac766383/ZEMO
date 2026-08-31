# ZEMO macOS Developer ID signing and notarization

External macOS beta builds must be signed with an Apple **Developer ID Application** certificate and notarized by Apple. The private-beta workflow is fail-closed: a workflow-dispatch release does not upload a distributable artifact unless code signing, notarization, stapling and Gatekeeper assessment all pass.

## Required GitHub Actions secrets

Configure these repository secrets before dispatching a macOS private beta:

- `MACOS_CERTIFICATE_P12_BASE64` — base64 of the exported Developer ID Application `.p12` certificate including its private key.
- `MACOS_CERTIFICATE_PASSWORD` — password used when exporting that `.p12`.
- `APPLE_API_ISSUER` — App Store Connect API issuer UUID.
- `APPLE_API_KEY` — App Store Connect API key ID.
- `APPLE_API_KEY_P8_BASE64` — base64 of the downloaded `AuthKey_<KEY_ID>.p8` private key.

The `.p12` and `.p8` files themselves must never be committed to the repository.

## Certificate preparation

1. Enroll in the Apple Developer Program.
2. Create a **Developer ID Application** certificate in the Apple Developer account.
3. Install it in Keychain Access on a trusted Mac and export the certificate **with its private key** as a password-protected `.p12`.
4. Base64-encode the `.p12` and store the result in `MACOS_CERTIFICATE_P12_BASE64`.

Example on macOS:

```bash
base64 -i DeveloperIDApplication.p12 | pbcopy
```

## Notarization API key

Create an App Store Connect API key with sufficient Developer access, record the issuer ID and key ID, then download the `.p8` key. Apple only allows the private key to be downloaded once.

Example on macOS:

```bash
base64 -i AuthKey_ABC123XYZ.p8 | pbcopy
```

Store the result in `APPLE_API_KEY_P8_BASE64`.

## Release qualification

For `workflow_dispatch` releases, `.github/workflows/zemo-macos-private-beta.yml` requires all of the following before packaging:

1. Developer ID identity imported into a temporary CI keychain.
2. Tauri build signed using that Developer ID identity.
3. Apple notarization credentials supplied to the Tauri bundler.
4. `codesign --verify --deep --strict` succeeds.
5. `xcrun stapler staple` and `xcrun stapler validate` succeed.
6. `spctl --assess --type execute` reports `source=Notarized Developer ID`.
7. Packaged-sidecar authentication and physical Apply/Undo tests pass.

If any of these checks fail, no external-user beta artifact is uploaded.

## Trusted private testing without Apple credentials

For a very small trusted tester group, the pull-request validation path may build and upload a macOS artifact without Developer ID notarization. This route still runs bundle inspection plus packaged-sidecar authentication and physical Apply/Undo tests. It is only for private testing: the official external release path above remains fail-closed and must not be weakened.

Private-test build marker: real One-Click Ranger pipeline after PR #45.
