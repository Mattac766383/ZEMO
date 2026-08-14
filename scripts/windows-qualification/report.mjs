/**
 * Structured Windows qualification report helpers.
 * Status vocabulary: PASS | FAIL | PARTIAL | NOT RUN
 * Never auto-promote skipped work to PASS.
 */

export const Status = Object.freeze({
  PASS: "PASS",
  FAIL: "FAIL",
  PARTIAL: "PARTIAL",
  NOT_RUN: "NOT RUN",
});

export function createReport({ hostIsWindows, startedAt = new Date() }) {
  return {
    title: "WINDOWS QUALIFICATION",
    startedAt: startedAt.toISOString(),
    finishedAt: null,
    hostIsWindows,
    nativeRuntime: hostIsWindows ? "RUN ATTEMPTED" : "NOT TESTED",
    environment: {},
    sections: {
      "BUILD PREP": [],
      "READ-ONLY": [],
      SEMANTIC: [],
      MONITORING: [],
      EXECUTOR: [],
      NTFS: [],
      ROLLBACK: [],
      INSTALLER: [],
      "SANDBOX SAFETY": [],
    },
  };
}

export function addCheck(report, section, name, status, detail = "") {
  if (!report.sections[section]) {
    throw new Error(`unknown qualification section: ${section}`);
  }
  if (!Object.values(Status).includes(status)) {
    throw new Error(`invalid status for ${name}: ${status}`);
  }
  report.sections[section].push({ name, status, detail: String(detail || "") });
}

export function sectionStatus(checks) {
  if (!checks.length) {
    return Status.NOT_RUN;
  }
  const statuses = new Set(checks.map((check) => check.status));
  if (statuses.has(Status.FAIL)) {
    return Status.FAIL;
  }
  if (statuses.has(Status.PARTIAL)) {
    return Status.PARTIAL;
  }
  if (statuses.has(Status.NOT_RUN) && !statuses.has(Status.PASS)) {
    return Status.NOT_RUN;
  }
  if (statuses.has(Status.NOT_RUN) && statuses.has(Status.PASS)) {
    return Status.PARTIAL;
  }
  return Status.PASS;
}

export function markNotRunSection(report, section, reason) {
  addCheck(report, section, "native Windows runtime", Status.NOT_RUN, reason);
}

export function formatReport(report) {
  report.finishedAt = new Date().toISOString();
  const lines = [];
  lines.push(report.title);
  lines.push("=".repeat(report.title.length));
  lines.push(`Started:  ${report.startedAt}`);
  lines.push(`Finished: ${report.finishedAt}`);
  lines.push(`Host OS Windows: ${report.hostIsWindows ? "yes" : "no"}`);
  lines.push(`NATIVE WINDOWS RUNTIME: ${report.nativeRuntime}`);
  lines.push("");
  lines.push("ENVIRONMENT:");
  for (const [key, value] of Object.entries(report.environment)) {
    lines.push(`  ${key}: ${value}`);
  }
  lines.push("");

  for (const [section, checks] of Object.entries(report.sections)) {
    const status = sectionStatus(checks);
    lines.push(`${section}: ${status}`);
    if (!checks.length) {
      lines.push("  (no checks recorded)");
    } else {
      for (const check of checks) {
        const detail = check.detail ? ` — ${check.detail}` : "";
        lines.push(`  [${check.status}] ${check.name}${detail}`);
      }
    }
    lines.push("");
  }

  lines.push("SUMMARY:");
  for (const [section, checks] of Object.entries(report.sections)) {
    lines.push(`  ${section}: ${sectionStatus(checks)}`);
  }
  lines.push(`  NATIVE WINDOWS RUNTIME: ${report.nativeRuntime}`);
  return `${lines.join("\n")}\n`;
}

export function overallPrepStatus(report) {
  const critical = [
    sectionStatus(report.sections["BUILD PREP"]),
    sectionStatus(report.sections["SANDBOX SAFETY"]),
    sectionStatus(report.sections.INSTALLER),
  ];
  if (critical.includes(Status.FAIL)) {
    return "FAIL";
  }
  if (critical.includes(Status.PARTIAL) || critical.includes(Status.NOT_RUN)) {
    return "PARTIAL";
  }
  return "PASS";
}
