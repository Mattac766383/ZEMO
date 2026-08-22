#!/usr/bin/env node
/**
 * Decide whether the Windows Apply-enabled installer may be built.
 *
 * Reads the existing M15-A harness JSON. No workflow input can force
 * apply_qualified=true. Compile-time feature enablement happens only in
 * the follow-up job when this script prints apply_qualified=true.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { sectionStatus, Status } from "../windows-qualification/report.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryDirectory = resolve(scriptDirectory, "../..");

const REQUIRED_PASS_SECTIONS = [
  "READ-ONLY",
  "MONITORING",
  "EXECUTOR",
  "NTFS",
  "ROLLBACK",
  "SANDBOX SAFETY",
];

function isKnownNonBlockingBuildPrepFailure(check) {
  return (
    check?.status === Status.FAIL &&
    check?.name === "cargo tree numkong features" &&
    /package ID specification [`']numkong[`'] did not match any packages/i.test(
      String(check?.detail || ""),
    )
  );
}

function buildPrepFailures(report) {
  const checks = report.sections?.["BUILD PREP"] || [];
  return {
    blocking: checks.filter(
      (check) => check?.status === Status.FAIL && !isKnownNonBlockingBuildPrepFailure(check),
    ),
    ignored: checks.filter(isKnownNonBlockingBuildPrepFailure),
  };
}

export function decideApplyQualification(report, extras = {}) {
  const sectionStatuses = {};
  for (const [name, checks] of Object.entries(report.sections || {})) {
    sectionStatuses[name] = sectionStatus(checks);
  }

  const filesystem = String(
    extras.filesystem ||
      report.environment?.["qualification sandbox filesystem"] ||
      report.environment?.filesystem ||
      "",
  ).trim();
  const ntfs = /^NTFS$/i.test(filesystem);
  const hostIsWindows = Boolean(report.hostIsWindows);
  const semantic = sectionStatuses.SEMANTIC || Status.NOT_RUN;
  const blockers = [];
  const buildPrep = buildPrepFailures(report);

  if (!hostIsWindows) {
    blockers.push("host is not Windows");
  }
  if (!ntfs) {
    blockers.push(`qualification volume is '${filesystem || "unknown"}', not NTFS`);
  }
  if (report.nativeRuntime !== "RUN ATTEMPTED") {
    blockers.push(`native runtime is ${report.nativeRuntime || "unknown"}`);
  }
  for (const section of REQUIRED_PASS_SECTIONS) {
    const status = sectionStatuses[section] || Status.NOT_RUN;
    if (status !== Status.PASS) {
      blockers.push(`${section} is ${status}`);
    }
  }
  if (semantic === Status.FAIL) {
    blockers.push("SEMANTIC failed");
  }
  if (buildPrep.blocking.length > 0) {
    blockers.push(
      `BUILD PREP failed: ${buildPrep.blocking.map((check) => check.name).join(", ")}`,
    );
  }

  const applyQualified = blockers.length === 0;
  return {
    apply_qualified: applyQualified,
    qualification_status: applyQualified ? "PASS" : "FAIL",
    filesystem: filesystem || "unknown",
    ntfs,
    hostIsWindows,
    nativeRuntime: report.nativeRuntime || "unknown",
    semantic,
    granite: extras.granite || process.env.ZEMO_GRANITE_STATUS || semantic,
    sectionStatuses,
    blockers,
    ignored_build_prep_failures: buildPrep.ignored.map((check) => ({
      name: check.name,
      reason: "obsolete optional dependency probe; numkong is absent from the resolved graph",
    })),
    required_pass_sections: REQUIRED_PASS_SECTIONS,
  };
}

function formatSummary(decision) {
  const lines = [
    "ZEMO WINDOWS QUALIFICATION SUMMARY",
    "==================================",
    `Apply qualified: ${decision.apply_qualified ? "YES" : "NO"}`,
    `Qualification: ${decision.qualification_status}`,
    `Host Windows: ${decision.hostIsWindows ? "yes" : "no"}`,
    `Filesystem: ${decision.filesystem}`,
    `NTFS: ${decision.ntfs ? "yes" : "no"}`,
    `Native runtime: ${decision.nativeRuntime}`,
    `Granite / SEMANTIC: ${decision.semantic}`,
    "",
    "SECTIONS:",
  ];
  for (const [name, status] of Object.entries(decision.sectionStatuses)) {
    lines.push(`  ${name}: ${status}`);
  }
  lines.push("");
  if (decision.ignored_build_prep_failures?.length) {
    lines.push("NON-BLOCKING BUILD PREP PROBES:");
    for (const ignored of decision.ignored_build_prep_failures) {
      lines.push(`  - ${ignored.name}: ${ignored.reason}`);
    }
    lines.push("");
  }
  if (decision.blockers.length) {
    lines.push("BLOCKERS:");
    for (const blocker of decision.blockers) {
      lines.push(`  - ${blocker}`);
    }
  } else {
    lines.push("BLOCKERS: none");
  }
  lines.push("");
  lines.push(
    decision.apply_qualified
      ? "Apply-enabled Windows beta may be built."
      : "Do not build or upload an Apply-enabled installer.",
  );
  return `${lines.join("\n")}\n`;
}

function appendGithubFile(filePath, lines) {
  if (!filePath) {
    return;
  }
  writeFileSync(filePath, `${lines.join("\n")}\n`, { flag: "a" });
}

function main() {
  const reportPath =
    process.argv[2] ||
    join(
      repositoryDirectory,
      "target/windows-qualification/windows-qualification-latest.json",
    );
  if (!existsSync(reportPath)) {
    throw new Error(`qualification report missing: ${reportPath}`);
  }
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  const decision = decideApplyQualification(report, {
    filesystem: process.env.ZEMO_WINDOWS_QUALIFICATION_FILESYSTEM,
    granite: process.env.ZEMO_GRANITE_STATUS,
  });
  const outDir = join(repositoryDirectory, "target/windows-qualification");
  mkdirSync(outDir, { recursive: true });
  const summaryPath = join(outDir, "qualification-summary.txt");
  const decisionPath = join(outDir, "apply-decision.json");
  const summary = formatSummary(decision);
  writeFileSync(summaryPath, summary);
  writeFileSync(decisionPath, `${JSON.stringify(decision, null, 2)}\n`);
  appendGithubFile(process.env.GITHUB_OUTPUT, [
    `apply_qualified=${decision.apply_qualified ? "true" : "false"}`,
    `qualification_status=${decision.qualification_status}`,
    `filesystem=${decision.filesystem}`,
    `semantic=${decision.semantic}`,
  ]);
  process.stdout.write(summary);
  process.stdout.write(`Decision written to ${decisionPath}\n`);
}

const invokedDirectly =
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (invokedDirectly) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error?.message || error}\n`);
    process.exit(1);
  }
}
