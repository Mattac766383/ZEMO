import {
  copyFileSync,
  existsSync,
  mkdirSync,
  renameSync,
  rmSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const repositoryDirectory = path.resolve(desktopDirectory, "../..");
const sidecarDirectory = path.join(desktopDirectory, "src-tauri", "binaries");
const cargoTargetDirectory = process.env.CARGO_TARGET_DIR
  ? path.resolve(repositoryDirectory, process.env.CARGO_TARGET_DIR)
  : path.join(repositoryDirectory, "target");

const argumentsList = process.argv.slice(2);
const checkOnly = argumentsList.includes("--check");
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
  commandOutput("rustc", ["--print", "host-tuple"]);

if (!/^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(targetTriple)) {
  fail(`unsafe or invalid Rust target triple: ${targetTriple}`);
}

const windowsTarget = targetTriple.includes("-windows-");
const macTarget = targetTriple.includes("-apple-darwin") || targetTriple.includes("-macos");
const extension = windowsTarget ? ".exe" : "";
const source = path.join(
  cargoTargetDirectory,
  targetTriple,
  "release",
  `operation-executor${extension}`,
);
const destination = path.join(
  sidecarDirectory,
  `operation-executor-${targetTriple}${extension}`,
);

if (checkOnly) {
  process.stdout.write(`${targetTriple}\n${destination}\n`);
  process.exit(0);
}

if (!windowsTarget && !macTarget) {
  fail(
    "operation-executor sidecar is configured for Windows and macOS targets only",
  );
}

run("cargo", [
  "build",
  "--release",
  "--package",
  "operation-executor",
  "--target",
  targetTriple,
]);

if (!existsSync(source)) {
  fail(`Cargo completed without producing the expected sidecar: ${source}`);
}

mkdirSync(sidecarDirectory, { recursive: true });
const temporary = `${destination}.tmp-${process.pid}`;
rmSync(temporary, { force: true });
copyFileSync(source, temporary);
rmSync(destination, { force: true });
renameSync(temporary, destination);
process.stdout.write(`Prepared ${destination}\n`);

function commandOutput(command, args) {
  const result = spawnSync(command, args, {
    cwd: repositoryDirectory,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.status !== 0) {
    fail(`${command} failed with status ${result.status ?? "unknown"}`);
  }
  const output = result.stdout.trim();
  if (!output) {
    fail(`${command} returned an empty target triple`);
  }
  return output;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repositoryDirectory,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    fail(`${command} failed with status ${result.status ?? "unknown"}`);
  }
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}
