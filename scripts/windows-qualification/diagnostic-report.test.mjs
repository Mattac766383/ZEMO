import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { Status } from "./report.mjs";
import {
  DIAGNOSTIC_SECTIONS,
  formatDiagnosticReport,
  mapQualificationToDiagnostic,
} from "./diagnostic-report.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));

test("diagnostic report contains every required section and keeps Apply disabled", () => {
  const qualification = {
    hostIsWindows: true,
    startedAt: "2026-01-01T00:00:00.000Z",
    environment: { filesystem: "NTFS" },
    sections: {
      "BUILD PREP": [{ name: "check", status: Status.PASS, detail: "" }],
      "READ-ONLY": [{ name: "link", status: Status.FAIL, detail: "LNK1169" }],
      SEMANTIC: [{ name: "ort", status: Status.FAIL, detail: "dll" }],
      MONITORING: [{ name: "create", status: Status.PASS, detail: "" }],
      EXECUTOR: [{ name: "protocol", status: Status.PASS, detail: "" }],
      NTFS: [{ name: "move", status: Status.FAIL, detail: "87" }],
      ROLLBACK: [{ name: "undo", status: Status.PASS, detail: "" }],
      INSTALLER: [{ name: "sidecar", status: Status.PASS, detail: "" }],
      "SANDBOX SAFETY": [{ name: "temp", status: Status.PASS, detail: "" }],
    },
  };
  const report = mapQualificationToDiagnostic(qualification, {
    qualificationDecision: "FAIL",
    stages: [
      { name: "ORT LOAD", status: "FAIL", detail: "onnxruntime.dll" },
      { name: "USEARCH LOAD", status: "PASS", detail: "" },
    ],
  });
  assert.equal(report.applyEnabled, false);
  assert.equal(report.qualificationDecision, "FAIL");
  for (const section of DIAGNOSTIC_SECTIONS) {
    assert.ok(Array.isArray(report.sections[section]), section);
  }
  const text = formatDiagnosticReport(report);
  assert.match(text, /WINDOWS APPLY: DISABLED/);
  assert.match(text, /QUALIFICATION DECISION: FAIL/);
  assert.match(text, /LNK1169/);
  assert.match(text, /onnxruntime\.dll/);
});

test("workflow collects diagnostics after failures and still fails qualification", () => {
  const yaml = readFileSync(
    join(scriptDirectory, "../../.github/workflows/zemo-windows-private-beta.yml"),
    "utf8",
  );
  assert.match(yaml, /ZEMO_WINDOWS_DIAGNOSTIC_ONLY: "0"/);
  assert.match(yaml, /continue-on-error: true/);
  assert.match(yaml, /windows-diagnostic-report\.txt/);
  assert.match(yaml, /windows-diagnostic-report\.json/);
  assert.match(yaml, /Fail if required qualification gates did not pass/);
  assert.match(yaml, /diagnostic_only != 'true'/);
  assert.match(yaml, /prepare-operation-executor-sidecar\.mjs/);
  assert.match(yaml, /LIBSQLITE3_FLAGS: "-DSQLCIPHER_OMIT_DLLMAIN"/);
  assert.match(yaml, /RUST_MIN_STACK: "268435456"/);
  assert.match(yaml, /RUSTFLAGS: "-C link-arg=\/STACK:268435456"/);
  assert.doesNotMatch(yaml, /FORCE:MULTIPLE/);
});
