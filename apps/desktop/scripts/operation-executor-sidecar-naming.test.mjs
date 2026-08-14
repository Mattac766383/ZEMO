import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MACOS_ARM64_TARGET,
  SIDECAR_BASE_NAME,
  TAURI_EXTERNAL_BIN,
  WINDOWS_MSVC_X64_TARGET,
  sidecarNaming,
} from "./operation-executor-sidecar-naming.mjs";

test("Windows x64 MSVC uses a single .exe and the MSVC triple", () => {
  const naming = sidecarNaming(WINDOWS_MSVC_X64_TARGET);
  assert.equal(naming.targetTriple, "x86_64-pc-windows-msvc");
  assert.equal(naming.windowsTarget, true);
  assert.equal(naming.extension, ".exe");
  assert.equal(
    naming.packagedFileName,
    "operation-executor-x86_64-pc-windows-msvc.exe",
  );
  assert.equal(naming.cargoFileName, "operation-executor.exe");
  assert.equal(naming.tauriExternalBin, "binaries/operation-executor");
  assert.equal(naming.packagedFileName.endsWith(".exe.exe"), false);
  assert.equal(naming.tauriExternalBin.endsWith(".exe"), false);
  assert.equal(naming.supported, true);
});

test("macOS arm64 keeps the existing sidecar name without .exe", () => {
  const naming = sidecarNaming(MACOS_ARM64_TARGET);
  assert.equal(naming.targetTriple, "aarch64-apple-darwin");
  assert.equal(naming.macTarget, true);
  assert.equal(naming.extension, "");
  assert.equal(
    naming.packagedFileName,
    "operation-executor-aarch64-apple-darwin",
  );
  assert.equal(naming.cargoFileName, "operation-executor");
  assert.equal(naming.tauriExternalBin, TAURI_EXTERNAL_BIN);
  assert.equal(naming.sidecarBase, SIDECAR_BASE_NAME);
  assert.equal(naming.supported, true);
});

test("does not treat the GNU Windows triple as the project default", () => {
  const gnu = sidecarNaming("x86_64-pc-windows-gnu");
  assert.equal(gnu.packagedFileName, "operation-executor-x86_64-pc-windows-gnu.exe");
  assert.notEqual(gnu.targetTriple, WINDOWS_MSVC_X64_TARGET);
  assert.notEqual(gnu.packagedFileName, sidecarNaming(WINDOWS_MSVC_X64_TARGET).packagedFileName);
});
