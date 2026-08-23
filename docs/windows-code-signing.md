# Windows code signing for ZEMO private beta

ZEMO's Windows private-beta workflow supports Authenticode signing without storing certificate material in the repository.

## GitHub secrets

Configure these repository Actions secrets:

- `WINDOWS_CERTIFICATE`: the code-signing `.pfx` encoded as base64.
- `WINDOWS_CERTIFICATE_PASSWORD`: the password used to protect/export the `.pfx`.
- `WINDOWS_TIMESTAMP_URL` (optional): RFC3161 timestamp endpoint. If omitted, the workflow uses `http://timestamp.digicert.com`.

Never commit a certificate, private key, PFX password, or base64 PFX to the repository.

## Encoding a PFX

On Windows, one supported option is:

```powershell
certutil -encode certificate.pfx certificate-base64.txt
```

Store the resulting text as the `WINDOWS_CERTIFICATE` secret and the PFX password as `WINDOWS_CERTIFICATE_PASSWORD`.

## CI behavior

When both required secrets are present, the Windows private-beta build:

1. decodes the PFX only on the ephemeral GitHub-hosted Windows runner;
2. imports it into `Cert:\CurrentUser\My`;
3. requires a private-key certificate with the Code Signing EKU;
4. derives its thumbprint automatically;
5. generates a temporary Tauri config overlay using SHA-256 and RFC3161 timestamping;
6. runs `tauri build` with that signing overlay;
7. verifies Authenticode on the produced ZEMO application and NSIS installer;
8. fails before upload if signing was configured but either required signature is invalid;
9. writes `authenticode-report.json` into the private-beta artifact and updates the beta manifest/build info to state the verified signing result.

If both required secrets are absent, the workflow can still create an unsigned private beta and clearly records that signing is not configured. If only one of the two required secrets is present, the build fails closed as a configuration error.

## SmartScreen

A valid Authenticode signature and a timestamp are important, but they do not automatically guarantee that Microsoft SmartScreen will show no warning. ZEMO therefore keeps `smartscreen_external_user_experience_qualified=false` until the external download/install experience has actually been verified.

Certificate reputation may need time to build. Modern OV/EV certificates can also be hardware- or cloud-backed and may not be exportable as a PFX. If the certificate issuer requires a hardware/cloud signing service, use Tauri's Windows `signCommand` route instead of this PFX path.
