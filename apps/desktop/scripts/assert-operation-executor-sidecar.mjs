/**
 * Preflight: the real Tauri sidecar must exist before desktop cargo check/build.
 * cargo check does not run Tauri beforeBuildCommand.
 *
 * Refuses empty placeholder binaries.
 */
import { existsSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { sidecarNaming } from "./operation-executor-sidecar-naming.mjs";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const repositoryDirectory = path.resolve(desktopDirectory, "../..");

export function sidecarPackagedPath(repositoryRoot, triple) {
  const resolved = sidecarNaming(triple);
  return path.join(
    repositoryRoot,
    "apps",
    "desktop",
    "src-tauri",
    "binaries",
    resolved.packagedFileName,
  );
}

export function evaluateSidecarPreflight(destinationPath) {
  if (!existsSync(destinationPath)) {
    return { ok: false, reason: `sidecar missing: ${destinationPath}` };
  }
  const stats = statSync(destinationPath);
  if (!stats.isFile()) {
    return { ok: false, reason: `sidecar is not a file: ${destinationPath}` };
  }
  if (stats.size === 0) {
    return {
      ok: false,
      reason: `sidecar is empty (refusing placeholder): ${destinationPath}`,
    };
  }
  return { ok: true, reason: `${destinationPath} (${stats.size} bytes)` };
}

function rustcHostTriple() {
  const printed = commandOutput("rustc", ["--print", "host-tuple"]);
  if (printed) {
    return printed;
  }
  const legacy = commandOutput("rustc", ["--print", "host-triple"]);
  if (legacy) {
    return legacy;
  }
  fail("rustc did not report a host target triple; pass --target");
}

function commandOutput(command, args) {
  const result = spawnSync(command, args, {
    cwd: repositoryDirectory,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    return "";
  }
  return (result.stdout || "").trim();
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

function main() {
  const argumentsList = process.argv.slice(2);
  const targetArgumentIndex = argumentsList.indexOf("--target");
  if (
    targetArgumentIndex >= 0 &&
    (targetArgumentIndex + 1 >= argumentsList.length ||
      argumentsList[targetArgumentIndex + 1].startsWith("-"))
  ) {
    fail("--target requires a Rust target triple");
  }

  const explicitTarget =
    targetArgumentIndex >= 0 ? argumentsList[targetArgumentIndex + 1] : undefined;
  const targetTriple =
    explicitTarget ??
    process.env.TAURI_ENV_TARGET_TRIPLE ??
    process.env.CARGO_BUILD_TARGET ??
    rustcHostTriple();

  let naming;
  try {
    naming = sidecarNaming(targetTriple);
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }

  const destination = sidecarPackagedPath(repositoryDirectory, targetTriple);
  const evaluation = evaluateSidecarPreflight(destination);

  process.stdout.write(
    [
      naming.targetTriple,
      destination,
      `expected packaged filename: ${naming.packagedFileName}`,
      `Tauri externalBin entry: ${naming.tauriExternalBin}`,
      evaluation.ok
        ? `preflight PASS: ${evaluation.reason}`
        : `preflight FAIL: ${evaluation.reason}`,
      "",
    ].join("\n"),
  );

  if (!evaluation.ok) {
    fail(
      [
        evaluation.reason,
        "cargo check/build does not run Tauri beforeBuildCommand.",
        "Prepare the real sidecar first:",
        `  npm run sidecar:prepare --workspace @working-name/desktop -- --target ${naming.targetTriple}`,
        "Do not create an empty placeholder .exe and do not commit generated binaries.",
      ].join("\n"),
    );
  }
}

const invokedDirectly =
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (invokedDirectly) {
  main();
}
