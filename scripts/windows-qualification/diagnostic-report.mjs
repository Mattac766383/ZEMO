/**
 * Consolidated Windows diagnostic report. Independent of Apply enablement.
 */

import { sectionStatus, Status } from "./report.mjs";

export const DIAGNOSTIC_SECTIONS = [
  "ENVIRONMENT",
  "COMPILATION",
  "VOLUME",
  "NTFS",
  "PATH IDENTITY",
  "LOCKS",
  "ACL",
  "REPARSE",
  "ROLLBACK",
  "MONITORING",
  "LINKER",
  "SEMANTIC",
  "ORT",
  "GRANITE",
  "USEARCH",
  "EXECUTOR",
  "CRASH RECOVERY",
  "INSTALLER PREP",
  "SANDBOX SAFETY",
];

export function emptyDiagnosticReport({ hostIsWindows, environment = {} } = {}) {
  const sections = {};
  for (const name of DIAGNOSTIC_SECTIONS) {
    sections[name] = [];
  }
  return {
    title: "WINDOWS DIAGNOSTIC REPORT",
    hostIsWindows: Boolean(hostIsWindows),
    applyEnabled: false,
    qualificationDecision: "FAIL",
    environment,
    sections,
    startedAt: new Date().toISOString(),
    finishedAt: null,
  };
}

export function addDiagnosticCheck(report, section, name, status, detail = "") {
  if (!report.sections[section]) {
    throw new Error(`unknown diagnostic section: ${section}`);
  }
  report.sections[section].push({
    name,
    status,
    detail: String(detail || ""),
  });
}

export function mapQualificationToDiagnostic(qualification, extras = {}) {
  const report = emptyDiagnosticReport({
    hostIsWindows: qualification.hostIsWindows,
    environment: qualification.environment || {},
  });
  report.startedAt = qualification.startedAt || report.startedAt;
  report.applyEnabled = false;
  report.qualificationDecision = extras.qualificationDecision || "FAIL";

  const mapped = extras.mappedChecks || [];
  for (const check of mapped) {
    addDiagnosticCheck(
      report,
      check.diagnostic,
      check.name,
      check.status,
      check.detail,
    );
  }

  copySection(qualification, "BUILD PREP", report, "COMPILATION");
  copySection(qualification, "READ-ONLY", report, "LINKER");
  copySection(qualification, "INSTALLER", report, "INSTALLER PREP");
  copySection(qualification, "SANDBOX SAFETY", report, "SANDBOX SAFETY");
  copySection(qualification, "MONITORING", report, "MONITORING");
  copySection(qualification, "SEMANTIC", report, "SEMANTIC");
  copySection(qualification, "EXECUTOR", report, "EXECUTOR");
  copySection(qualification, "ROLLBACK", report, "ROLLBACK");
  copySection(qualification, "NTFS", report, "NTFS");

  if (extras.stages?.length) {
    for (const stage of extras.stages) {
      const status = stage.status === "PASS" ? Status.PASS : Status.FAIL;
      addDiagnosticCheck(report, "SEMANTIC", `stage ${stage.name}`, status, stage.detail);
      if (["ORT LOAD", "TOKENIZER", "ONNX SESSION"].includes(stage.name)) {
        addDiagnosticCheck(report, "ORT", stage.name, status, stage.detail);
      }
      if (["MODEL ASSETS", "CHECKSUM", "GRANITE EMBEDDING", "DIMENSION CHECK"].includes(stage.name)) {
        addDiagnosticCheck(report, "GRANITE", stage.name, status, stage.detail);
      }
      if (["USEARCH LOAD", "INDEX CREATE", "INSERT", "QUERY", "PERSIST", "RELOAD"].includes(stage.name)) {
        addDiagnosticCheck(report, "USEARCH", stage.name, status, stage.detail);
      }
    }
  }

  ensureSectionHasEntry(report, "ENVIRONMENT", "host", qualification.hostIsWindows ? Status.PASS : Status.FAIL, JSON.stringify(qualification.environment || {}));
  return report;
}

function copySection(source, from, target, to) {
  for (const check of source.sections?.[from] || []) {
    addDiagnosticCheck(target, to, check.name, check.status, check.detail);
  }
}

function ensureSectionHasEntry(report, section, name, status, detail) {
  if (!report.sections[section].length) {
    addDiagnosticCheck(report, section, name, status, detail);
  }
}

export function formatDiagnosticReport(report) {
  report.finishedAt = new Date().toISOString();
  const lines = [
    report.title,
    "=".repeat(report.title.length),
    `Started:  ${report.startedAt}`,
    `Finished: ${report.finishedAt}`,
    `Host OS Windows: ${report.hostIsWindows ? "yes" : "no"}`,
    `WINDOWS APPLY: DISABLED`,
    `QUALIFICATION DECISION: ${report.qualificationDecision}`,
    "",
    "ENVIRONMENT:",
  ];
  for (const [key, value] of Object.entries(report.environment || {})) {
    lines.push(`  ${key}: ${value}`);
  }
  lines.push("");
  for (const section of DIAGNOSTIC_SECTIONS) {
    const checks = report.sections[section] || [];
    lines.push(`${section}: ${sectionStatus(checks)}`);
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
  return `${lines.join("\n")}\n`;
}
