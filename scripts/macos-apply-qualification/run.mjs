#!/usr/bin/env node
/**
 * M18 Step 2 — packaged macOS Apply qualification harness.
 *
 * Builds (unless --skip-build) an unsigned 0.1.0-beta.3-m18 .app/.dmg,
 * inspects the bundled sidecar, and runs packaged-executor Apply tests
 * against dedicated supremacy-m18-step2-sandbox-* folders only.
 *
 * Does not touch Documents/Desktop/Downloads.
 * Does not request Full Disk Access.
 * Does not disable TCC/Gatekeeper.
 * Does not distribute the artifact.
 */

import { spawnSync } from "node:child_process";
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
const desktopDirectory = path.join(repositoryDirectory, "apps/desktop");
const artifactsDirectory = path.join(
  repositoryDirectory,
  "artifacts/m18-step2",
);
const packVersion = "0.1.0-beta.3-m18";
const args = new Set(process.argv.slice(2));
const skipBuild = args.has("--skip-build");
const skipGui = args.has("--skip-gui");

function run(command, argv, options = {}) {
  const result = spawnSync(command, argv, {
    cwd: repositoryDirectory,
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
    path.join(
      cargoTarget,
      "release/bundle/macos/ZEMO.app",
    ),
    path.join(
      cargoTarget,
      "aarch64-apple-darwin/release/bundle/macos/ZEMO.app",
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
    throw new Error("release ZEMO.app was not found after build");
  }
  const appPath = path.join(artifactsDirectory, "ZEMO.app");
  rmSync(appPath, { recursive: true, force: true });
  cpSync(builtApp, appPath, { recursive: true });

  const builtDmg = findBundleDmg();
  const dmgPath = path.join(
    artifactsDirectory,
    `ZEMO-${packVersion}-arm64.dmg`,
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
      ? `${dmgChecksum}  ZEMO-${packVersion}-arm64.dmg\n`
      : "dmg not produced\n",
  );

  const testEnv = {
    ...process.env,
    WORKING_NAME_PACKAGED_APP: appPath,
  };
  const tests = run(
    "cargo",
    [
      "test",
      "-p",
      "desktop",
      "--lib",
      "packaged_macos_apply",
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

  let tcc = {
    attempted: !skipGui,
    promptObserved: false,
    persistenceAfterRelaunch: "NOT OBSERVED",
    denial: "NOT OBSERVED",
    revocation: "POSIX revoke tested; TCC Files-and-Folders revoke NOT OBSERVED",
    notes: [],
  };
  if (!skipGui) {
    const isolatedHome = mkdtempSync(
      path.join(tmpdir(), "supremacy-m18-step2-home-"),
    );
    const launchBinary = existsSync(
      path.join(appPath, "Contents/MacOS/desktop"),
    )
      ? path.join(appPath, "Contents/MacOS/desktop")
      : path.join(appPath, "Contents/MacOS/ZEMO");
    const launch = spawnSync(
      launchBinary,
      [],
      {
        env: { ...process.env, HOME: isolatedHome },
        encoding: "utf8",
        timeout: 5000,
        killSignal: "SIGTERM",
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    tcc.notes.push(
      `isolated HOME launch status=${launch.status} timedOut=${launch.error?.code === "ETIMEDOUT"} (folder picker not driven; no Accessibility automation)`,
    );
    const tccQuery = run(
      "sqlite3",
      [
        `${process.env.HOME}/Library/Application Support/com.apple.TCC/TCC.db`,
        "select service,client,auth_value from access where client like '%workingname%' or client like '%Working Name%';",
      ],
      { capture: true, allowFail: true },
    );
    if (tccQuery.status === 0 && (tccQuery.stdout || "").trim()) {
      tcc.notes.push(`user TCC.db rows:\n${tccQuery.stdout}`);
    } else {
      tcc.notes.push(
        "user TCC.db not readable without Full Disk Access; not requested",
      );
    }
    tcc.notes.push(
      "Protected-folder TCC prompts were not exercised: qualification roots stay outside Documents/Desktop/Downloads.",
    );
    rmSync(isolatedHome, { recursive: true, force: true });
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
    tcc,
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
