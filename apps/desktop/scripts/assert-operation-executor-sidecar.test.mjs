import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { WINDOWS_MSVC_X64_TARGET } from "./operation-executor-sidecar-naming.mjs";
import {
  evaluateSidecarPreflight,
  sidecarPackagedPath,
} from "./assert-operation-executor-sidecar.mjs";

const repositoryDirectory = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);

test("Windows sidecar destination uses the canonical Tauri packaged name", () => {
  const destination = sidecarPackagedPath(
    repositoryDirectory,
    WINDOWS_MSVC_X64_TARGET,
  );
  assert.equal(
    path.basename(destination),
    "operation-executor-x86_64-pc-windows-msvc.exe",
  );
  assert.ok(destination.endsWith(path.join("src-tauri", "binaries", path.basename(destination))));
  assert.equal(destination.endsWith(".exe.exe"), false);
});

test("preflight refuses a missing, directory, or empty placeholder sidecar", () => {
  const root = mkdtempSync(path.join(tmpdir(), "zemo-sidecar-preflight-"));
  const missing = path.join(root, "missing.exe");
  assert.equal(evaluateSidecarPreflight(missing).ok, false);
  assert.match(evaluateSidecarPreflight(missing).reason, /missing/);

  const directory = path.join(root, "dir.exe");
  mkdirSync(directory);
  assert.equal(evaluateSidecarPreflight(directory).ok, false);
  assert.match(evaluateSidecarPreflight(directory).reason, /not a file/);

  const empty = path.join(root, "empty.exe");
  writeFileSync(empty, "");
  assert.equal(evaluateSidecarPreflight(empty).ok, false);
  assert.match(evaluateSidecarPreflight(empty).reason, /empty/);

  const real = path.join(root, "real.exe");
  writeFileSync(real, "MZ-not-a-committed-binary-just-a-nonzero-probe");
  const pass = evaluateSidecarPreflight(real);
  assert.equal(pass.ok, true);
  assert.match(pass.reason, /bytes/);
});
