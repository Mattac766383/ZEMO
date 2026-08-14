#!/usr/bin/env node
/**
 * M18 Step 2.1 — packaged macOS Apply P1 closure harness.
 *
 * Builds an unsigned 0.1.0-beta.3-m18.1 .app/.dmg, runs focused packaged
 * executor tests, and drives the real NSOpenPanel folder-selection flow
 * inside an isolated HOME.
 *
 * Does not touch Documents/Desktop/Downloads contents for Apply.
 * Does not request Full Disk Access.
 * Does not inspect TCC.db as a success criterion.
 * Does not disable TCC/Gatekeeper/SIP.
 * Does not distribute the artifact.
 */

import { spawn, spawnSync } from "node:child_process";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const repositoryDirectory = path.resolve(scriptDirectory, "../..");
const artifactsDirectory = path.join(
  repositoryDirectory,
  "artifacts/m18-step2.1",
);
const packVersion = "0.1.0-beta.3-m18.1";
const args = new Set(process.argv.slice(2));
const skipBuild = args.has("--skip-build");
const skipGui = args.has("--skip-gui");

function run(command, argv, options = {}) {
  const result = spawnSync(command, argv, {
    cwd: options.cwd ?? repositoryDirectory,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
    env: options.env ?? process.env,
  });
  if (result.status !== 0 && !options.allowFail) {
    const detail = options.capture
      ? `${result.stderr || result.stdout || ""}`.trim()
      : "";
    throw new Error(
      `${command} ${argv.join(" ")} failed (${result.status})${detail ? `\n${detail}` : ""}`,
    );
  }
  return result;
}

function sha256File(filePath) {
  const hash = createHash("sha256");
  hash.update(readFileSync(filePath));
  return hash.digest("hex");
}

function commandOutput(command, argv) {
  const result = run(command, argv, { capture: true, allowFail: true });
  return `${result.stdout || ""}${result.stderr || ""}`.trim();
}

function findBundleApp() {
  const cargoTarget =
    process.env.CARGO_TARGET_DIR || path.join(repositoryDirectory, "target");
  const candidates = [
    path.join(cargoTarget, "release/bundle/macos/Working Name.app"),
    path.join(
      cargoTarget,
      "aarch64-apple-darwin/release/bundle/macos/Working Name.app",
    ),
  ];
  return candidates.find((candidate) => existsSync(candidate));
}

function findBundleDmg() {
  const cargoTarget =
    process.env.CARGO_TARGET_DIR || path.join(repositoryDirectory, "target");
  const directories = [
    path.join(cargoTarget, "release/bundle/dmg"),
    path.join(cargoTarget, "aarch64-apple-darwin/release/bundle/dmg"),
  ];
  for (const directory of directories) {
    if (!existsSync(directory)) {
      continue;
    }
    const listing = commandOutput("ls", [directory]);
    const dmg = listing
      .split("\n")
      .map((line) => line.trim())
      .find((line) => line.endsWith(".dmg"));
    if (dmg) {
      return path.join(directory, dmg);
    }
  }
  return undefined;
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function waitForFile(filePath, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (existsSync(filePath)) {
      try {
        return JSON.parse(readFileSync(filePath, "utf8"));
      } catch {
        // file is still being written
      }
    }
    sleep(250);
  }
  return null;
}

function confirmOpenPanel() {
  return run(
    "osascript",
    [
      "-e",
      'tell application "System Events" to keystroke return',
    ],
    { capture: true, allowFail: true },
  );
}

function cancelOpenPanel() {
  return run(
    "osascript",
    [
      "-e",
      'tell application "System Events" to keystroke "w" using command down',
    ],
    { capture: true, allowFail: true },
  );
}

function launchQualifiedApp(appPath, env, timeoutMs) {
  const launchBinary = existsSync(path.join(appPath, "Contents/MacOS/desktop"))
    ? path.join(appPath, "Contents/MacOS/desktop")
    : path.join(appPath, "Contents/MacOS/Working Name");
  const child = spawn(launchBinary, [], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const killer = setTimeout(() => {
    child.kill("SIGTERM");
  }, timeoutMs);
  return {
    child,
    stop() {
      clearTimeout(killer);
      if (!child.killed) {
        child.kill("SIGTERM");
      }
    },
  };
}

function driveNsopenpanel(appPath) {
  const isolatedHome = mkdtempSync(
    path.join(tmpdir(), "supremacy-m18-step21-home-"),
  );
  const sandbox = mkdtempSync(
    path.join(tmpdir(), "supremacy-m18-step2-sandbox-"),
  );
  writeFileSync(path.join(sandbox, "qualify.txt"), "nsopenpanel-fixture\n");
  const dataDir = path.join(
    isolatedHome,
    "Library/Application Support/com.workingname.organizer",
  );
  const notes = [];
  const env = {
    ...process.env,
    HOME: isolatedHome,
    WORKING_NAME_QUALIFY_NSOPENPANEL: sandbox,
  };

  const first = launchQualifiedApp(appPath, env, 25000);
  sleep(1500);
  const confirm = confirmOpenPanel();
  notes.push(
    `NSOpenPanel confirm osascript status=${confirm.status} stderr=${(confirm.stderr || "").trim()}`,
  );
  const selected = waitForFile(
    path.join(dataDir, "qualify-nsopenpanel.json"),
    18000,
  );
  first.stop();
  sleep(500);

  const relaunchEnv = {
    ...process.env,
    HOME: isolatedHome,
    WORKING_NAME_QUALIFY_RELAUNCH: "1",
  };
  const second = launchQualifiedApp(appPath, relaunchEnv, 15000);
  const relaunch = waitForFile(path.join(dataDir, "qualify-relaunch.json"), 12000);
  second.stop();
  sleep(500);

  const cancelHome = mkdtempSync(
    path.join(tmpdir(), "supremacy-m18-step21-cancel-"),
  );
  const cancel = launchQualifiedApp(
    appPath,
    {
      ...process.env,
      HOME: cancelHome,
      WORKING_NAME_QUALIFY_NSOPENPANEL: sandbox,
    },
    15000,
  );
  sleep(1500);
  const cancelScript = cancelOpenPanel();
  notes.push(
    `NSOpenPanel cancel osascript status=${cancelScript.status} stderr=${(cancelScript.stderr || "").trim()}`,
  );
  const cancelled = waitForFile(
    path.join(
      cancelHome,
      "Library/Application Support/com.workingname.organizer/qualify-nsopenpanel.json",
    ),
    8000,
  );
  cancel.stop();

  const observation = {
    isolatedHome,
    sandbox,
    selected,
    relaunch,
    cancelled,
    notes,
    nativePrompt: "NOT REQUIRED for temp-folder selection (no Files & Folders alert expected)",
    tccDbInspected: false,
  };
  writeFileSync(
    path.join(artifactsDirectory, "nsopenpanel-observation.json"),
    `${JSON.stringify(observation, null, 2)}\n`,
  );
  rmSync(isolatedHome, { recursive: true, force: true });
  rmSync(cancelHome, { recursive: true, force: true });
  rmSync(sandbox, { recursive: true, force: true });
  return observation;
}

function main() {
  mkdirSync(artifactsDirectory, { recursive: true });
  const commit = commandOutput("git", ["rev-parse", "HEAD"]);
  const architecture = commandOutput("uname", ["-m"]);
  const macos = commandOutput("sw_vers", ["-productVersion"]);

  if (!skipBuild) {
    run("npm", ["run", "sidecar:prepare", "--workspace", "@working-name/desktop"]);
    run("npm", ["run", "tauri", "--", "build"]);
  }

  const builtApp = findBundleApp();
  if (!builtApp) {
    throw new Error("release Working Name.app was not found after build");
  }
  const appPath = path.join(artifactsDirectory, "Working Name.app");
  rmSync(appPath, { recursive: true, force: true });
  cpSync(builtApp, appPath, { recursive: true });

  const builtDmg = findBundleDmg();
  const dmgPath = path.join(
    artifactsDirectory,
    `Working-Name-${packVersion}-arm64.dmg`,
  );
  if (builtDmg) {
    copyFileSync(builtDmg, dmgPath);
  }

  const sidecar = path.join(appPath, "Contents/MacOS/operation-executor");
  const sidecarExists = existsSync(sidecar);
  const entitlements = commandOutput("codesign", [
    "-d",
    "--entitlements",
    "-",
    "--xml",
    appPath,
  ]);
  const codesign = commandOutput("codesign", ["-dv", "--verbose=2", appPath]);
  const dmgChecksum = existsSync(dmgPath) ? sha256File(dmgPath) : "NOT BUILT";
  const appChecksum = commandOutput("shasum", ["-a", "256", sidecar]).split(
    /\s+/,
  )[0];

  const buildInfo = [
    `pack=${packVersion}`,
    `app_version=0.1.0`,
    `architecture=${architecture}`,
    `macos=${macos}`,
    `commit=${commit}`,
    `app=${appPath}`,
    `dmg=${existsSync(dmgPath) ? dmgPath : "NOT BUILT"}`,
    `dmg_sha256=${dmgChecksum}`,
    `sidecar=${sidecar}`,
    `sidecar_present=${sidecarExists}`,
    `sidecar_sha256=${appChecksum || "missing"}`,
    `signing=ad-hoc`,
    `notarization=NOT CONFIGURED`,
    `distribution=DO NOT DISTRIBUTE`,
    `full_disk_access_requested=no`,
    `app_sandbox_entitlement=${/com\\.apple\\.security\\.app-sandbox/.test(entitlements)}`,
  ].join("\n");
  writeFileSync(path.join(artifactsDirectory, "BUILDINFO.txt"), `${buildInfo}\n`);
  writeFileSync(
    path.join(artifactsDirectory, "SHA256SUMS.txt"),
    existsSync(dmgPath)
      ? `${dmgChecksum}  Working-Name-${packVersion}-arm64.dmg\n`
      : "dmg not produced\n",
  );

  const testEnv = {
    ...process.env,
    WORKING_NAME_PACKAGED_APP: appPath,
  };
  const focusedTests = [
    "packaged_case_only",
    "packaged_controlled_crash",
    "packaged_crash_after_ack",
    "packaged_no_overwrite",
    "packaged_revoked_access",
    "packaged_relaunch_journal",
    "packaged_unicode",
  ];
  const tests = run(
    "cargo",
    [
      "test",
      "-p",
      "desktop",
      "--lib",
      focusedTests.join("|"),
      "--",
      "--ignored",
      "--nocapture",
      "--test-threads=1",
    ],
    { env: testEnv, capture: true, allowFail: true },
  );
  writeFileSync(
    path.join(artifactsDirectory, "packaged-executor-tests.txt"),
    `${tests.stdout || ""}\n${tests.stderr || ""}`,
  );

  let nsopenpanel = { attempted: !skipGui };
  if (!skipGui) {
    nsopenpanel = { attempted: true, ...driveNsopenpanel(appPath) };
  }

  const report = {
    packVersion,
    commit,
    architecture,
    appPath,
    dmgPath: existsSync(dmgPath) ? dmgPath : null,
    sidecarExists,
    entitlementsHasAppSandbox: /com\.apple\.security\.app-sandbox/.test(
      entitlements,
    ),
    codesign,
    testsExit: tests.status,
    nsopenpanel,
  };
  writeFileSync(
    path.join(artifactsDirectory, "qualification-report.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  if (tests.status !== 0) {
    process.exit(tests.status ?? 1);
  }
}

try {
  main();
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : error}\n`);
  process.exit(1);
}
