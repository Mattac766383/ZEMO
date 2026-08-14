/**
 * Canonical operation-executor sidecar names for Tauri externalBin.
 *
 * Tauri config lists the base without target triple or .exe:
 *   binaries/operation-executor
 * The file on disk must be:
 *   operation-executor-<triple>        (macOS)
 *   operation-executor-<triple>.exe    (Windows)
 */

export const SIDECAR_BASE_NAME = "operation-executor";
export const TAURI_EXTERNAL_BIN = "binaries/operation-executor";
export const WINDOWS_MSVC_X64_TARGET = "x86_64-pc-windows-msvc";
export const MACOS_ARM64_TARGET = "aarch64-apple-darwin";

const TARGET_TRIPLE = /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/;

export function sidecarNaming(targetTriple) {
  if (typeof targetTriple !== "string" || !TARGET_TRIPLE.test(targetTriple)) {
    throw new Error(`unsafe or invalid Rust target triple: ${targetTriple}`);
  }
  const windowsTarget = targetTriple.includes("-windows-");
  const macTarget =
    targetTriple.includes("-apple-darwin") || targetTriple.includes("-macos");
  const extension = windowsTarget ? ".exe" : "";
  const packagedFileName = `${SIDECAR_BASE_NAME}-${targetTriple}${extension}`;
  if (packagedFileName.endsWith(".exe.exe")) {
    throw new Error(`refusing double .exe sidecar name: ${packagedFileName}`);
  }
  return {
    targetTriple,
    windowsTarget,
    macTarget,
    supported: windowsTarget || macTarget,
    extension,
    sidecarBase: SIDECAR_BASE_NAME,
    cargoFileName: `${SIDECAR_BASE_NAME}${extension}`,
    packagedFileName,
    tauriExternalBin: TAURI_EXTERNAL_BIN,
  };
}
