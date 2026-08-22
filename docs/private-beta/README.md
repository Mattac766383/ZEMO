# ZEMO — Private beta distribution

This folder documents the controlled external-testing path. It is not a public release process.

## What a tester receives

Every distributable must contain:

- the ZEMO installer/application archive;
- `SHA256SUMS.txt`;
- `BUILDINFO.txt`;
- `README-FIRST.txt`;
- `beta-manifest.json`.

The manifest is intentionally portable: it records the beta tag, platform, architecture, commit, artifact name, checksum, qualification state and signing state. It must not contain runner paths or user file data.

## Windows

Use the **ZEMO Windows Private Beta** workflow.

Windows Apply builds are produced only after the native NTFS qualification decides `apply_qualified=true`. If Apply is not qualified, the workflow can produce the safer propose-only build instead.

Current limitations that must remain explicit to testers:

- Windows signing is not claimed configured;
- SmartScreen external-user experience is not qualified;
- the first test must use copies in a small test folder;
- Windows Defender must stay enabled.

## macOS

Use the **ZEMO macOS Private Beta** workflow and provide a tag such as `0.1.0-beta.7`.

Before packaging, the workflow:

1. builds the real `ZEMO.app` with the packaged sidecar;
2. verifies the application bundle;
3. runs packaged sidecar authentication and physical Apply/Undo tests;
4. creates a ZIP that preserves macOS bundle metadata;
5. verifies the ZIP SHA-256 before upload.

Current limitations that must remain explicit to testers:

- signing is ad-hoc only;
- Apple notarization is not configured;
- Gatekeeper external-user experience is not qualified;
- testers must never be told to disable Gatekeeper.

## First external test protocol

For the first 5–10 testers, ask them to use this sequence:

1. install/open ZEMO without developer assistance;
2. use a small folder containing copies of files;
3. scan and inspect the proposed organization;
4. apply only a small approved proposal;
5. test Undo;
6. run a semantic search;
7. inspect the local beta diagnostic on Home if something fails.

The local beta diagnostic is intended to share coarse counters only. Testers should never send document contents, file names, file paths or search queries as part of the standard diagnostic.

## Promotion rule

Do not call a build "external beta ready" merely because it compiles. A candidate should have:

- Core CI green;
- One-Click acceptance green;
- packaged runtime qualification green for the target platform;
- npm high/critical audit green;
- a reproducible checksum and manifest;
- tester-facing installation/safety instructions.
