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
import { sidecarNaming } from "./operation-executor-sidecar-naming.mjs";

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
  rustcHostTriple();

let naming;
try {
  naming = sidecarNaming(targetTriple);
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}

const source = path.join(
  cargoTargetDirectory,
  naming.targetTriple,
  "release",
  naming.cargoFileName,
);
const destination = path.join(sidecarDirectory, naming.packagedFileName);

if (checkOnly) {
  const found = existsSync(destination) ? destination : "not present";
  const lines = [
    naming.targetTriple,
    destination,
    `host platform: ${process.platform}`,
    `target: ${naming.targetTriple}`,
    `expected sidecar base: ${naming.sidecarBase}`,
    `expected packaged filename: ${naming.packagedFileName}`,
    `actual found: ${found}`,
    `Tauri externalBin entry: ${naming.tauriExternalBin}`,
    "binary required: no (configuration/naming check)",
  ];
  process.stdout.write(`${lines.join("\n")}\n`);
  if (!naming.supported) {
    fail("operation-executor sidecar is configured for Windows and macOS targets only");
  }
  process.exit(0);
}

if (!naming.supported) {
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
  naming.targetTriple,
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

function rustcHostTriple() {
  const printed = commandOutput("rustc", ["--print", "host-tuple"]);
  if (printed) {
    return printed;
  }
  const legacy = commandOutput("rustc", ["--print", "host-triple"]);
  if (legacy) {
    return legacy;
  }
  fail("rustc did not report a host target triple");
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
