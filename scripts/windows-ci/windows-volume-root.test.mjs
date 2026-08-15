import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { windowsVolumeRoot } from "./windows-volume-root.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));

test("parses GitHub Actions runner temp without requiring the drive to exist", () => {
  const parsed = windowsVolumeRoot("D:\\a\\_temp\\zemo-windows-qualification");
  assert.deepEqual(parsed, { volumeRoot: "D:\\", driveLetter: "D" });
});

test("does not hardcode D: and accepts another runner drive", () => {
  const parsed = windowsVolumeRoot("C:\\a\\_temp\\zemo-windows-qualification");
  assert.deepEqual(parsed, { volumeRoot: "C:\\", driveLetter: "C" });
});

test("parses verbatim Win32 roots used after canonicalize", () => {
  const parsed = windowsVolumeRoot("\\\\?\\D:\\a\\_temp\\zemo-windows-qualification");
  assert.deepEqual(parsed, { volumeRoot: "D:\\", driveLetter: "D" });
});

test("accepts forward slashes after the drive", () => {
  const parsed = windowsVolumeRoot("E:/a/_temp/zemo-windows-qualification");
  assert.deepEqual(parsed, { volumeRoot: "E:\\", driveLetter: "E" });
});

test("returns null for non-Windows paths instead of assuming NTFS", () => {
  assert.equal(windowsVolumeRoot("/var/folders/tmp/zemo-windows-qualification"), null);
  assert.equal(windowsVolumeRoot(""), null);
});

test("qualification harness does not use PSDrive.FileSystem", () => {
  const harness = readFileSync(
    join(scriptDirectory, "../windows-qualification/run.mjs"),
    "utf8",
  );
  assert.equal(harness.includes(".PSDrive"), false);
  assert.match(harness, /DriveInfo/);
});

test("verify-ntfs.ps1 uses volume-root detection and not PSDrive.FileSystem", () => {
  const source = readFileSync(join(scriptDirectory, "verify-ntfs.ps1"), "utf8");
  assert.equal(source.includes(".PSDrive"), false);
  assert.match(source, /GetPathRoot/);
  assert.match(source, /Get-Volume -DriveLetter/);
  assert.match(source, /DriveInfo/);
  assert.match(source, /Win32_LogicalDisk/);
  assert.doesNotMatch(source, /D:\\a\\_temp/);
  assert.match(source, /could not be determined/);
});
