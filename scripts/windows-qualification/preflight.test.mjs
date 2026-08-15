import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { compileSuites } from "./suites.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");

test("desktop cargo check is ordered after sidecar prepare in the harness", () => {
  const harness = readFileSync(join(root, "scripts/windows-qualification/run.mjs"), "utf8");
  const prepareAt = harness.indexOf("prepareWindowsSidecar(report)");
  const compileAt = harness.indexOf("runCargoSuites(report, compileSuites)");
  assert.ok(prepareAt >= 0, "harness must prepare the Windows sidecar");
  assert.ok(compileAt >= 0, "harness must run compile suites");
  assert.ok(prepareAt < compileAt, "sidecar must be prepared before cargo check -p desktop");
  assert.ok(harness.includes("assert-operation-executor-sidecar.mjs"));
  const desktop = compileSuites.find((suite) => suite.name === "cargo check -p desktop");
  assert.ok(desktop?.requiresSidecar);
});

test("workflow prepares the real sidecar before cargo check -p desktop", () => {
  const yaml = readFileSync(
    join(root, ".github/workflows/zemo-windows-private-beta.yml"),
    "utf8",
  );
  const prepareAt = yaml.indexOf("prepare-operation-executor-sidecar.mjs");
  const desktopAt = yaml.indexOf("cargo check -p desktop");
  assert.ok(prepareAt >= 0);
  assert.ok(desktopAt >= 0);
  assert.ok(prepareAt < desktopAt);
  assert.ok(yaml.includes("assert-operation-executor-sidecar.mjs"));
  assert.ok(yaml.includes("LIBSQLITE3_FLAGS: \"-DSQLCIPHER_OMIT_DLLMAIN\""));
  assert.ok(yaml.includes("cargo tree -p application -e features -i libsqlite3-sys"));
  assert.ok(yaml.includes("cargo tree -p search -e features -i usearch"));
  assert.doesNotMatch(yaml, /FORCE:MULTIPLE/);
  assert.doesNotMatch(yaml, /empty placeholder|\.exe\.exe/);
});

test("SQLCipher static Windows builds omit DllMain without weakening SQLCipher", () => {
  const cargoConfig = readFileSync(join(root, ".cargo/config.toml"), "utf8");
  const persistence = readFileSync(join(root, "crates/persistence/Cargo.toml"), "utf8");
  const persistenceBuild = readFileSync(join(root, "crates/persistence/build.rs"), "utf8");
  assert.match(cargoConfig, /LIBSQLITE3_FLAGS/);
  assert.match(cargoConfig, /SQLCIPHER_OMIT_DLLMAIN/);
  assert.doesNotMatch(cargoConfig, /FORCE:MULTIPLE/);
  assert.match(persistence, /bundled-sqlcipher-vendored-openssl/);
  assert.match(persistenceBuild, /SQLCIPHER_OMIT_DLLMAIN/);
});
