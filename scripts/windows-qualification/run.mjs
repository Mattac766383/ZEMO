#!/usr/bin/env node
/**
 * M15-A Windows qualification harness (preparation + gated runtime).
 *
 * On non-Windows hosts:
 *   - verifies packaging / build-prep configuration
 *   - records ENVIRONMENT
 *   - marks native runtime sections NOT RUN (never PASS)
 *
 * On Windows hosts:
 *   - runs the dedicated Windows qualification cargo/UI suites
 *   - mutations remain isolated under temporary supremacy-m15-sandbox-* roots
 *
 * This harness does NOT claim native Windows PASS unless tests actually ran.
 */

import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  mkdtempSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  Status,
  addCheck,
  createReport,
  formatReport,
  markNotRunSection,
  overallPrepStatus,
  sectionStatus,
} from "./report.mjs";
import { sidecarNaming } from "../../apps/desktop/scripts/operation-executor-sidecar-naming.mjs";
import { extractDiagnostics, formatCapturedFailure } from "./capture.mjs";
import {
  compileSuites,
  executorSuites,
  monitoringSuites,
  ntfsSuites,
  pathIdentitySuites,
  readOnlySuites,
  sandboxSuites,
  semanticSuites,
  volumeSuites,
} from "./suites.mjs";
import {
  formatDiagnosticReport,
  mapQualificationToDiagnostic,
} from "./diagnostic-report.mjs";
import { decideApplyQualification } from "../windows-ci/decide-apply-qualification.mjs";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const repositoryDirectory = path.resolve(scriptDirectory, "../..");
const reportsDirectory = path.join(repositoryDirectory, "target", "windows-qualification");
const hostIsWindows = process.platform === "win32";
const windowsTarget = "x86_64-pc-windows-msvc";

const args = new Set(process.argv.slice(2));
const skipCargoCheck = args.has("--skip-cargo-check");
const skipRuntime = args.has("--prep-only");
const nativeFirst = args.has("--native-first");
const diagnostic =
  args.has("--diagnostic") || process.env.ZEMO_WINDOWS_DIAGNOSTIC_ONLY === "1";
const jsonOut = args.has("--json") || diagnostic;

function main() {
  mkdirSync(reportsDirectory, { recursive: true });
  const report = createReport({ hostIsWindows });
  report._diagnosticMapped = [];
  report._semanticStages = [];
  try {
    collectEnvironment(report);
    runBuildPrep(report);
    runInstallerPrep(report);
    runSandboxSafetyPrep(report);

    if (!hostIsWindows || skipRuntime) {
      const reason = !hostIsWindows
        ? "host is not Windows; native runtime qualification requires a real Windows machine/runner"
        : "--prep-only requested; native runtime suites not executed";
      for (const section of [
        "READ-ONLY",
        "SEMANTIC",
        "MONITORING",
        "EXECUTOR",
        "NTFS",
        "ROLLBACK",
      ]) {
        markNotRunSection(report, section, reason);
      }
      report.nativeRuntime = "NOT TESTED";
    } else {
      report.nativeRuntime = "RUN ATTEMPTED";
      runWindowsRuntime(report, { nativeFirst, diagnostic });
    }
  } catch (error) {
    addCheck(
      report,
      "BUILD PREP",
      "diagnostic harness exception",
      Status.FAIL,
      String(error?.stack || error?.message || error),
    );
  }

  writeDiagnosticArtifacts(report);

  const text = formatReport(report);
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const reportPath = path.join(reportsDirectory, `windows-qualification-${stamp}.txt`);
  writeFileSync(reportPath, text);
  writeFileSync(
    path.join(reportsDirectory, "windows-qualification-latest.txt"),
    text,
  );
  if (jsonOut) {
    const publicReport = { ...report };
    delete publicReport._diagnosticMapped;
    delete publicReport._semanticStages;
    writeFileSync(
      path.join(reportsDirectory, "windows-qualification-latest.json"),
      `${JSON.stringify(publicReport, null, 2)}\n`,
    );
  }

  process.stdout.write(text);
  process.stdout.write(`\nReport written to ${reportPath}\n`);
  process.stdout.write(
    `M15-A prep overall (non-runtime): ${overallPrepStatus(report)}\n`,
  );

  const failedPrep = ["BUILD PREP", "INSTALLER", "SANDBOX SAFETY"].some(
    (section) => sectionStatus(report.sections[section]) === Status.FAIL,
  );
  const failedRuntime =
    hostIsWindows &&
    !skipRuntime &&
    ["READ-ONLY", "SEMANTIC", "MONITORING", "EXECUTOR", "NTFS", "ROLLBACK"].some(
      (section) => sectionStatus(report.sections[section]) === Status.FAIL,
    );
  process.exit(failedPrep || failedRuntime ? 1 : 0);
}

function collectEnvironment(report) {
  report.environment = {
    platform: process.platform,
    arch: process.arch,
    node: process.version,
    cwd: repositoryDirectory,
    "rustc host": commandText("rustc", ["-vV"])
      .split("\n")
      .find((line) => line.startsWith("host:"))
      ?.slice(6)
      .trim() || "unknown",
    "rustc version": commandText("rustc", ["--version"]) || "unavailable",
    "cargo version": commandText("cargo", ["--version"]) || "unavailable",
    "windows target installed": rustupHasTarget(windowsTarget) ? "yes" : "no",
    "app build": readJson(path.join(repositoryDirectory, "apps/desktop/package.json"))
      ?.version || "unknown",
    "ORT crate": dependencyVersion("crates/search/Cargo.toml", "ort") || "unknown",
    "USearch crate": dependencyVersion("crates/search/Cargo.toml", "usearch") || "unknown",
    "model env SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR": process.env
      .SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR
      ? "set"
      : "unset",
  };

    if (hostIsWindows) {
    report.environment["Windows version"] =
      commandText("cmd.exe", ["/c", "ver"]) || "unknown";
    report.environment.CPU =
      commandText("powershell.exe", [
        "-NoProfile",
        "-Command",
        "(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)",
      ]) || "unknown";
    report.environment.RAM =
      commandText("powershell.exe", [
        "-NoProfile",
        "-Command",
        "[math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1).ToString() + ' GiB'",
      ]) || "unknown";
    report.environment.filesystem = windowsFilesystemForPath(process.cwd()) || "unknown";
    const sandboxRoot = process.env.ZEMO_WINDOWS_QUALIFICATION_ROOT;
    if (sandboxRoot) {
      report.environment["qualification sandbox"] = sandboxRoot;
      report.environment["qualification sandbox filesystem"] =
        process.env.ZEMO_WINDOWS_QUALIFICATION_FILESYSTEM ||
        windowsFilesystemForPath(sandboxRoot) ||
        "unknown";
    }
  } else {
    report.environment["Windows version"] = "NOT RUN (non-Windows host)";
    report.environment.CPU = commandText("sysctl", ["-n", "machdep.cpu.brand_string"]) ||
      commandText("uname", ["-m"]) ||
      "unknown";
    report.environment.RAM = "see host (non-Windows)";
    report.environment.filesystem = "see host (non-Windows)";
  }
}

function runBuildPrep(report) {
  const windowsConf = path.join(
    repositoryDirectory,
    "apps/desktop/src-tauri/tauri.windows.conf.json",
  );
  const tauriConf = path.join(
    repositoryDirectory,
    "apps/desktop/src-tauri/tauri.conf.json",
  );
  const searchToml = readText("crates/search/Cargo.toml");
  const persistenceToml = readText("crates/persistence/Cargo.toml");
  const desktopToml = readText("apps/desktop/src-tauri/Cargo.toml");
  const executorToml = readText("workers/operation-executor/Cargo.toml");
  const sidecarScript = readText(
    "apps/desktop/scripts/prepare-operation-executor-sidecar.mjs",
  );
  const platformWindowsToml = readText("crates/platform-windows/Cargo.toml");

  addCheck(
    report,
    "BUILD PREP",
    "Tauri Windows overlay config present",
    existsSync(windowsConf) ? Status.PASS : Status.FAIL,
    windowsConf,
  );

  const windowsJson = existsSync(windowsConf)
    ? JSON.parse(readFileSync(windowsConf, "utf8"))
    : null;
  const hasSidecar =
    Array.isArray(windowsJson?.bundle?.externalBin) &&
    windowsJson.bundle.externalBin.some((entry) =>
      String(entry).includes("operation-executor"),
    );
  addCheck(
    report,
    "BUILD PREP",
    "Windows externalBin includes operation-executor sidecar",
    hasSidecar ? Status.PASS : Status.FAIL,
  );

  const sidecarPrepare =
    String(windowsJson?.build?.beforeBuildCommand || "").includes("sidecar:prepare");
  addCheck(
    report,
    "BUILD PREP",
    "Windows beforeBuildCommand prepares sidecar",
    sidecarPrepare ? Status.PASS : Status.FAIL,
    windowsJson?.build?.beforeBuildCommand || "missing",
  );

  addCheck(
    report,
    "BUILD PREP",
    "Base Tauri config present",
    existsSync(tauriConf) ? Status.PASS : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "ORT download-binaries feature enabled",
    searchToml.includes("download-binaries") ? Status.PASS : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "USearch dependency pinned for in-process ANN",
    /usearch\s*=/.test(searchToml) ? Status.PASS : Status.FAIL,
    dependencyVersion("crates/search/Cargo.toml", "usearch") || "",
  );

  addCheck(
    report,
    "BUILD PREP",
    "SQLCipher bundled via rusqlite",
    persistenceToml.includes("bundled-sqlcipher-vendored-openssl")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "Desktop depends on platform-windows only on Windows",
    desktopToml.includes("cfg(windows)") &&
      desktopToml.includes("platform-windows")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "operation-executor mutation feature is platform-gated",
    executorToml.includes("cfg(windows)") &&
      executorToml.includes("cfg(target_os = \"macos\")") &&
      executorToml.includes('features = ["mutation"]')
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "platform-windows mutation feature is opt-in (default off)",
    platformWindowsToml.includes("mutation = []") &&
      platformWindowsToml.includes("default = []")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "Sidecar prepare script accepts only Windows and macOS targets",
    sidecarScript.includes("Windows and macOS targets only") ? Status.PASS : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "notify watcher dependency present (monitoring)",
    readText("crates/platform/Cargo.toml").includes("notify")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "Granite model manifest present",
    existsSync(
      path.join(
        repositoryDirectory,
        "models/manifests/granite-embedding-97m-multilingual-r2.v1.json",
      ),
    )
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "BUILD PREP",
    "NTFS qualification suite present",
    existsSync(
      path.join(
        repositoryDirectory,
        "crates/platform-windows/tests/ntfs_qualification.rs",
      ),
    )
      ? Status.PASS
      : Status.FAIL,
  );

  const targetInstalled = rustupHasTarget(windowsTarget);
  addCheck(
    report,
    "BUILD PREP",
    `Rust target ${windowsTarget} installed`,
    targetInstalled ? Status.PASS : Status.PARTIAL,
    targetInstalled
      ? "installed"
      : `run: rustup target add ${windowsTarget}`,
  );

  if (skipCargoCheck) {
    addCheck(
      report,
      "BUILD PREP",
      "Windows-target cargo check",
      Status.NOT_RUN,
      "--skip-cargo-check",
    );
  } else if (!targetInstalled) {
    addCheck(
      report,
      "BUILD PREP",
      "Windows-target cargo check",
      Status.NOT_RUN,
      "target not installed",
    );
  } else {
    const check = run(
      "cargo",
      [
        "check",
        "-p",
        "platform-windows",
        "-p",
        "operations",
        "-p",
        "operation-executor",
        "-p",
        "search",
        "-p",
        "ipc-contracts",
        "--target",
        windowsTarget,
      ],
      { allowFailure: true },
    );
    if (check.status === 0) {
      addCheck(
        report,
        "BUILD PREP",
        "Windows-target cargo check (selected crates)",
        Status.PASS,
      );
    } else {
      const output = `${check.stdout}\n${check.stderr}`;
      const missingLinker =
        /lib\.exe|link\.exe|MSVC|LNK|linker.*not found|unable to find|dlltool/i.test(
          output,
        );
      addCheck(
        report,
        "BUILD PREP",
        "Windows-target cargo check (selected crates)",
        missingLinker ? Status.PARTIAL : Status.FAIL,
        missingLinker
          ? "target present but MSVC/link tooling unavailable on this host (expected on macOS without full cross-link); not a native runtime result"
          : failureTail(output),
      );
    }
  }

  if (hostIsWindows && !skipCargoCheck) {
    recordCargoCheck(
      report,
      "BUILD PREP",
      "Native cargo check -p platform-windows",
      ["check", "-p", "platform-windows"],
    );
    recordCargoCheck(
      report,
      "BUILD PREP",
      "Native cargo check -p platform-windows --features mutation",
      ["check", "-p", "platform-windows", "--features", "mutation"],
    );
  }
}

function runInstallerPrep(report) {
  const docs = readText("docs/qualification/windows.md");
  const manifestsReadme = readText("models/manifests/README.md");
  const expectedSidecar = sidecarNaming(windowsTarget);
  const sidecarCheck = run(
    "npm",
    [
      "run",
      "sidecar:check",
      "--workspace",
      "@working-name/desktop",
      "--",
      "--target",
      windowsTarget,
    ],
    { allowFailure: true },
  );
  const sidecarOutput = `${sidecarCheck.stdout}\n${sidecarCheck.stderr}`;

  addCheck(
    report,
    "INSTALLER",
    "Qualification documentation present",
    docs.includes("WINDOWS QUALIFICATION") || docs.includes("Windows qualification")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "INSTALLER",
    "Model packaging docs state no developer env required for normal users",
    manifestsReadme.includes("normal users do not require") ||
      manifestsReadme.includes("Production (normal user)")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "INSTALLER",
    "sidecar:check reports Windows target naming",
    sidecarCheck.status === 0 &&
      sidecarOutput.includes(windowsTarget) &&
      sidecarOutput.includes(expectedSidecar.packagedFileName) &&
      sidecarOutput.includes(expectedSidecar.tauriExternalBin)
      ? Status.PASS
      : Status.FAIL,
    truncate(sidecarOutput.trim(), 400),
  );

  addCheck(
    report,
    "INSTALLER",
    "Release-like Windows install checklist documented",
    docs.includes("sidecar") &&
      docs.includes("ORT") &&
      docs.includes("ANN") &&
      docs.includes("DB writable")
      ? Status.PASS
      : Status.FAIL,
  );

  if (hostIsWindows) {
    const probe = probeWritableAppPaths();
    addCheck(
      report,
      "INSTALLER",
      "Model / ANN / DB app-data paths writable",
      probe.ok ? Status.PASS : Status.FAIL,
      probe.detail,
    );
  } else {
    addCheck(
      report,
      "INSTALLER",
      "Model / ANN / DB app-data paths writable",
      Status.NOT_RUN,
      "requires Windows host local app data",
    );
  }
}

function runSandboxSafetyPrep(report) {
  const forbidden = ["Documents", "Desktop", "Downloads"];
  const harness = readText("scripts/windows-qualification/run.mjs");
  const ntfs = readText("crates/platform-windows/tests/ntfs_qualification.rs");
  const nativePaths = existsSync(
    path.join(
      repositoryDirectory,
      "crates/platform-windows/tests/windows_native_paths.rs",
    ),
  )
    ? readText("crates/platform-windows/tests/windows_native_paths.rs")
    : "";

  addCheck(
    report,
    "SANDBOX SAFETY",
    "Harness forbids user profile corpus directories",
    forbidden.every((name) => harness.includes(name))
      ? Status.PASS
      : Status.FAIL,
    "Documents/Desktop/Downloads must be named in containment policy",
  );

  addCheck(
    report,
    "SANDBOX SAFETY",
    "NTFS suite asserts temporary sandbox containment",
    ntfs.includes("temporary root") || ntfs.includes("starts_with(&temporary_root)")
      ? Status.PASS
      : Status.FAIL,
  );

  addCheck(
    report,
    "SANDBOX SAFETY",
    "M15 sandbox prefix reserved for qualification fixtures",
    nativePaths.includes("supremacy-m15-sandbox-") ||
      harness.includes("supremacy-m15-sandbox-")
      ? Status.PASS
      : Status.FAIL,
  );

  // Live containment probe: create a temp sandbox and ensure it is under OS temp.
  const root = mkdtempSync(path.join(tmpdir(), "supremacy-m15-sandbox-"));
  try {
    const temporaryRoot = path.resolve(tmpdir());
    const escaped = forbidden.some((name) =>
      root.toLowerCase().includes(`${path.sep}${name.toLowerCase()}${path.sep}`),
    );
    const contained =
      path.resolve(root).startsWith(temporaryRoot) && !escaped;
    addCheck(
      report,
      "SANDBOX SAFETY",
      "Live temp sandbox containment probe",
      contained ? Status.PASS : Status.FAIL,
      root,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function qualificationVolumeIsNtfs(report) {
  const sandboxFs = report.environment["qualification sandbox filesystem"];
  const hostFs = report.environment.filesystem;
  const filesystem = String(sandboxFs || hostFs || "").trim();
  return /^NTFS$/i.test(filesystem);
}

function mutationContainment(report) {
  const root =
    process.env.ZEMO_WINDOWS_QUALIFICATION_ROOT ||
    process.env.RUNNER_TEMP ||
    process.env.TEMP ||
    "";
  const forbidden = ["Documents", "Desktop", "Downloads"];
  const escaped = forbidden.some((name) =>
    String(root)
      .toLowerCase()
      .includes(`\\${name.toLowerCase()}\\`),
  );
  if (!root || escaped) {
    return {
      ok: false,
      reason: `mutation containment not proven (root='${root || "missing"}')`,
    };
  }
  if (!qualificationVolumeIsNtfs(report)) {
    return {
      ok: false,
      reason: `qualification volume is not NTFS (${
        report.environment["qualification sandbox filesystem"] ||
        report.environment.filesystem ||
        "unknown"
      })`,
    };
  }
  return { ok: true, reason: root };
}

function runWindowsRuntime(report, { nativeFirst = false, diagnostic = false } = {}) {
  report._diagnosticMapped = [];
  report._semanticStages = [];
  const modelDir = process.env.SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR;
  const containment = mutationContainment(report);
  addCheck(
    report,
    "NTFS",
    "Qualification volume is NTFS",
    qualificationVolumeIsNtfs(report) ? Status.PASS : Status.FAIL,
    report.environment["qualification sandbox filesystem"] ||
      report.environment.filesystem ||
      "filesystem unknown",
  );
  addCheck(
    report,
    "SANDBOX SAFETY",
    "Mutation containment under runner temp",
    containment.ok ? Status.PASS : Status.FAIL,
    containment.reason,
  );

  if (diagnostic) {
    runCargoSuites(report, compileSuites);
    runCargoSuites(report, [
      {
        name: "platform-windows lib tests (compile + path parsing)",
        args: ["test", "-p", "platform-windows", "--lib", "--", "--nocapture"],
        section: "BUILD PREP",
        diagnostic: "COMPILATION",
      },
    ]);
  } else {
    runCargoSuites(report, [
      {
        name: "platform-windows lib tests (compile + path parsing)",
        args: ["test", "-p", "platform-windows", "--lib", "--", "--nocapture"],
        section: "NTFS",
        diagnostic: "COMPILATION",
      },
    ]);
  }

  // Volume diagnostic is independent of later NTFS mutation results.
  runCargoSuites(report, volumeSuites);
  runCargoSuites(report, monitoringSuites);
  runCargoSuites(report, sandboxSuites);

  if (containment.ok) {
    runCargoSuites(report, ntfsSuites);
    runCargoSuites(report, pathIdentitySuites);
    runCargoSuites(report, [
      {
        name: "Windows error taxonomy",
        args: [
          "test",
          "-p",
          "platform-windows",
          "--test",
          "windows_error_taxonomy",
          "--",
          "--nocapture",
        ],
        section: "NTFS",
        diagnostic: "NTFS",
      },
    ]);
  } else {
    const reason = `mutation skipped: ${containment.reason}`;
    addCheck(report, "NTFS", "Native NTFS Apply suites", Status.FAIL, reason);
    addCheck(report, "ROLLBACK", "Native rollback / recovery suites", Status.FAIL, reason);
    addCheck(report, "EXECUTOR", "Native mutation suites", Status.FAIL, reason);
  }

  if (nativeFirst) {
    const reason =
      "--native-first: application linker, semantic, and executor suites not executed";
    for (const section of ["READ-ONLY", "SEMANTIC", "EXECUTOR"]) {
      markNotRunSection(report, section, reason);
    }
    return;
  }

  // Linker / read-only / semantic continue even when NTFS mutation was skipped.
  runCargoSuites(report, readOnlySuites);
  const ui = run("npm", ["run", "test:ui"], { allowFailure: true, logName: "desktop-ui" });
  addCheck(
    report,
    "READ-ONLY",
    "Desktop UI smoke (Vitest)",
    ui.status === 0 ? Status.PASS : Status.FAIL,
    ui.status === 0
      ? "vitest ok"
      : formatCapturedFailure({
          command: "npm run test:ui",
          exitCode: ui.status,
          output: `${ui.stdout}\n${ui.stderr}`,
          logPath: ui.logPath,
        }),
  );

  if (containment.ok) {
    runCargoSuites(report, executorSuites);
  }

  if (!modelDir) {
    addCheck(
      report,
      "SEMANTIC",
      "Granite / ORT / ANN real-model suite",
      Status.NOT_RUN,
      "SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR unset; refuse to mark PASS",
    );
  } else {
    runCargoSuites(report, semanticSuites);
  }
}

function failureTail(output) {
  return formatCapturedFailure({
    command: "cargo",
    exitCode: 1,
    output,
  });
}

function recordCargoCheck(report, section, name, cargoArgs) {
  runCargoSuites(report, [
    {
      name,
      args: cargoArgs,
      section,
      diagnostic: "COMPILATION",
    },
  ]);
}

function runCargoSuites(report, suites) {
  for (const suite of suites) {
    const result = run("cargo", suite.args, {
      allowFailure: true,
      logName: suite.name,
    });
    const full = `${result.stdout}\n${result.stderr}`.trim();
    const captured = extractDiagnostics(full);
    if (captured.stages.length) {
      report._semanticStages = [
        ...(report._semanticStages || []),
        ...captured.stages,
      ];
    }
    const status = result.status === 0 ? Status.PASS : Status.FAIL;
    const detail =
      status === Status.PASS
        ? "cargo ok"
        : formatCapturedFailure({
            command: `cargo ${suite.args.join(" ")}`,
            exitCode: result.status,
            output: full,
            logPath: result.logPath,
          });
    addCheck(report, suite.section, suite.name, status, detail);
    report._diagnosticMapped = report._diagnosticMapped || [];
    report._diagnosticMapped.push({
      diagnostic: suite.diagnostic || suite.section,
      name: suite.name,
      status,
      detail,
    });
  }
}

function writeDiagnosticArtifacts(report) {
  let qualificationDecision = "FAIL";
  try {
    const decision = decideApplyQualification(report, {
      filesystem: process.env.ZEMO_WINDOWS_QUALIFICATION_FILESYSTEM,
    });
    qualificationDecision = decision.qualification_status || "FAIL";
  } catch (error) {
    qualificationDecision = "FAIL";
    addCheck(
      report,
      "BUILD PREP",
      "qualification decision",
      Status.FAIL,
      String(error?.message || error),
    );
  }
  const diagnostic = mapQualificationToDiagnostic(report, {
    qualificationDecision,
    mappedChecks: report._diagnosticMapped || [],
    stages: report._semanticStages || [],
  });
  const text = formatDiagnosticReport(diagnostic);
  writeFileSync(path.join(reportsDirectory, "windows-diagnostic-report.txt"), text);
  writeFileSync(
    path.join(reportsDirectory, "windows-diagnostic-report.json"),
    `${JSON.stringify(diagnostic, null, 2)}\n`,
  );
}

function probeWritableAppPaths() {
  const base = process.env.LOCALAPPDATA || process.env.TEMP || tmpdir();
  const root = path.join(base, "WorkingName", "windows-qualification-probe");
  try {
    mkdirSync(path.join(root, "models", "embeddings"), { recursive: true });
    mkdirSync(path.join(root, "models", "embeddings", "ann"), { recursive: true });
    mkdirSync(path.join(root, "db"), { recursive: true });
    writeFileSync(path.join(root, "models", "embeddings", ".write-test"), "ok");
    writeFileSync(path.join(root, "models", "embeddings", "ann", ".write-test"), "ok");
    writeFileSync(path.join(root, "db", ".write-test"), "ok");
    rmSync(root, { recursive: true, force: true });
    return { ok: true, detail: base };
  } catch (error) {
    return { ok: false, detail: String(error) };
  }
}

function windowsFilesystemForPath(targetPath) {
  if (!targetPath) {
    return "";
  }
  const escaped = String(targetPath).replace(/'/g, "''");
  // DriveInfo.DriveFormat is the volume type. Do not read the PowerShell
  // provider name; it is often empty on GitHub runner temp paths.
  return commandText("powershell.exe", [
    "-NoProfile",
    "-Command",
    `$p = [System.IO.Path]::GetFullPath('${escaped}'); $root = [System.IO.Path]::GetPathRoot($p); [System.IO.DriveInfo]::new($root).DriveFormat`,
  ]);
}

function rustupHasTarget(target) {
  const output = commandText("rustup", ["target", "list", "--installed"]);
  return output
    .split("\n")
    .map((line) => line.trim())
    .includes(target);
}

function dependencyVersion(relativeToml, name) {
  const text = readText(relativeToml);
  const match = text.match(
    new RegExp(`${name}\\s*=\\s*(?:\\{[^}]*version\\s*=\\s*)?[\"']([^\"']+)[\"']`),
  );
  return match?.[1];
}

function readText(relative) {
  const absolute = path.join(repositoryDirectory, relative);
  if (!existsSync(absolute)) {
    return "";
  }
  return readFileSync(absolute, "utf8");
}

function readJson(absolute) {
  try {
    return JSON.parse(readFileSync(absolute, "utf8"));
  } catch {
    return null;
  }
}

function commandText(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: repositoryDirectory,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    return "";
  }
  return (result.stdout || "").trim();
}

function run(command, commandArgs, { allowFailure = false, logName } = {}) {
  const isWindowsNpm =
    process.platform === "win32" && (command === "npm" || command === "npx");
  const result = spawnSync(command, commandArgs, {
    cwd: repositoryDirectory,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: process.env,
    shell: isWindowsNpm,
    windowsHide: true,
    maxBuffer: 32 * 1024 * 1024,
  });
  const spawnError = result.error ? `${result.error.message}\n` : "";
  const stdout = result.stdout || "";
  const stderr = `${spawnError}${result.stderr || ""}`;
  let logPath = "";
  if (logName) {
    const logsDirectory = path.join(reportsDirectory, "logs");
    mkdirSync(logsDirectory, { recursive: true });
    const safe = String(logName)
      .replace(/[^A-Za-z0-9._-]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 80);
    logPath = path.join(logsDirectory, `${safe || "command"}.log`);
    writeFileSync(logPath, `command: ${command} ${commandArgs.join(" ")}\nexit: ${result.status ?? 1}\n\n${stdout}\n${stderr}`);
  }
  if (!allowFailure && result.status !== 0) {
    throw new Error(
      `${command} ${commandArgs.join(" ")} failed: ${
        spawnError || result.stderr || result.stdout
      }`,
    );
  }
  return {
    status: result.status ?? 1,
    stdout,
    stderr,
    logPath,
  };
}

function truncate(value, max) {
  if (value.length <= max) {
    return value;
  }
  return `${value.slice(0, max)}…`;
}

main();
