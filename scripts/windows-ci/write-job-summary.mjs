#!/usr/bin/env node
/**
 * Append an honest ZEMO Windows private-beta summary to GITHUB_STEP_SUMMARY.
 */

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryDirectory = resolve(scriptDirectory, "../..");

function readJson(filePath) {
  if (!filePath || !existsSync(filePath)) {
    return null;
  }
  try {
    return JSON.parse(readFileSync(filePath, "utf8"));
  } catch {
    return null;
  }
}

function statusOr(value, fallback = "NOT RUN") {
  return value || fallback;
}

function fromNtfs(decision, whenPass) {
  const ntfs = decision?.sectionStatuses?.NTFS;
  if (ntfs === "PASS") {
    return whenPass;
  }
  return statusOr(ntfs);
}

function fromRecovery(decision) {
  const executor = decision?.sectionStatuses?.EXECUTOR;
  const rollback = decision?.sectionStatuses?.ROLLBACK;
  if (executor === "PASS" && rollback === "PASS") {
    return "PASS";
  }
  if (executor === "FAIL" || rollback === "FAIL") {
    return "FAIL";
  }
  return statusOr(executor || rollback);
}

function main() {
  const decision =
    readJson(process.argv[2]) ||
    readJson(join(repositoryDirectory, "target/windows-qualification/apply-decision.json"));
  const pack =
    readJson(process.argv[3]) ||
    readJson(join(repositoryDirectory, "target/windows-qualification/package-result.json"));
  const granite =
    readJson(process.env.ZEMO_GRANITE_STATUS_FILE) ||
    readJson(join(process.env.ZEMO_PINNED_MODEL_CACHE || "", "granite-status.json"));

  const environment = process.env.ZEMO_CI_ENVIRONMENT_STATUS || "NOT RUN";
  const frontend = process.env.ZEMO_CI_FRONTEND_STATUS || "NOT RUN";
  const rust = process.env.ZEMO_CI_RUST_STATUS || "NOT RUN";
  const apply = decision?.apply_qualified
    ? "PASS"
    : decision
      ? "FAIL"
      : "NOT RUN";
  const semantic = statusOr(decision?.semantic);
  const graniteStatus = statusOr(granite?.status || process.env.ZEMO_GRANITE_STATUS || semantic);
  const usearch =
    graniteStatus === "PASS" || semantic === "PASS"
      ? "PASS"
      : semantic === "FAIL" || graniteStatus === "FAIL"
        ? "FAIL"
        : "PARTIAL";

  const lines = [
    "## ZEMO WINDOWS PRIVATE BETA",
    "",
    `| Gate | Status |`,
    `| --- | --- |`,
    `| Environment | ${environment} |`,
    `| Frontend | ${frontend} |`,
    `| Rust | ${rust} |`,
    `| NTFS | ${statusOr(decision?.sectionStatuses?.NTFS)} |`,
    `| Executor | ${statusOr(decision?.sectionStatuses?.EXECUTOR)} |`,
    `| Locks | ${fromNtfs(decision, "PASS")} |`,
    `| ACL | ${fromNtfs(decision, "PASS")} |`,
    `| Reparse | ${fromNtfs(decision, "PASS")} |`,
    `| Crash recovery | ${fromRecovery(decision)} |`,
    `| Monitoring | ${statusOr(decision?.sectionStatuses?.MONITORING)} |`,
    `| Granite | ${graniteStatus} |`,
    `| USearch | ${usearch} |`,
    `| Installer | ${statusOr(pack?.installer_status)} |`,
    `| Apply qualification | ${apply} |`,
    `| Artifact | ${statusOr(pack?.artifact_status)} |`,
    "",
  ];

  if (decision?.blockers?.length) {
    lines.push("Apply blockers:");
    for (const blocker of decision.blockers) {
      lines.push(`- ${blocker}`);
    }
    lines.push("");
  }
  if (pack?.installer_name) {
    lines.push(`Installer file: \`${pack.installer_name}\``);
  }
  if (pack?.notes) {
    lines.push(pack.notes);
  }
  lines.push("");
  lines.push("WINDOWS SIGNING: NOT CONFIGURED unless a secret was detected without being printed.");
  lines.push("SMARTSCREEN USER EXPERIENCE: NOT QUALIFIED");
  lines.push("GUI interaction: NOT TESTED on the GitHub runner.");
  lines.push("");

  const text = `${lines.join("\n")}\n`;
  if (process.env.GITHUB_STEP_SUMMARY) {
    writeFileSync(process.env.GITHUB_STEP_SUMMARY, text, { flag: "a" });
  }
  process.stdout.write(text);
}

main();
