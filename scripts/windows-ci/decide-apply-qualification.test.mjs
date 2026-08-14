import assert from "node:assert/strict";
import { test } from "node:test";
import { decideApplyQualification } from "./decide-apply-qualification.mjs";
import { Status } from "../windows-qualification/report.mjs";

function passingReport(overrides = {}) {
  const pass = [{ name: "ok", status: Status.PASS, detail: "" }];
  return {
    hostIsWindows: true,
    nativeRuntime: "RUN ATTEMPTED",
    environment: {
      filesystem: "NTFS",
      "qualification sandbox filesystem": "NTFS",
    },
    sections: {
      "BUILD PREP": pass,
      "READ-ONLY": pass,
      SEMANTIC: [{ name: "granite", status: Status.NOT_RUN, detail: "unset" }],
      MONITORING: pass,
      EXECUTOR: pass,
      NTFS: pass,
      ROLLBACK: pass,
      INSTALLER: pass,
      "SANDBOX SAFETY": pass,
    },
    ...overrides,
  };
}

test("native NTFS safety PASS unlocks Apply even if Granite was not run", () => {
  const decision = decideApplyQualification(passingReport());
  assert.equal(decision.apply_qualified, true);
  assert.equal(decision.qualification_status, "PASS");
  assert.deepEqual(decision.blockers, []);
});

test("SEMANTIC FAIL blocks Apply", () => {
  const report = passingReport();
  report.sections.SEMANTIC = [{ name: "granite", status: Status.FAIL, detail: "load" }];
  const decision = decideApplyQualification(report);
  assert.equal(decision.apply_qualified, false);
  assert.ok(decision.blockers.some((item) => item.includes("SEMANTIC")));
});

test("non-NTFS volume cannot unlock Apply", () => {
  const report = passingReport();
  report.environment.filesystem = "FAT32";
  report.environment["qualification sandbox filesystem"] = "FAT32";
  const decision = decideApplyQualification(report);
  assert.equal(decision.apply_qualified, false);
  assert.ok(decision.blockers.some((item) => item.includes("FAT32")));
});

test("NTFS section PARTIAL cannot unlock Apply", () => {
  const report = passingReport();
  report.sections.NTFS = [{ name: "suite", status: Status.PARTIAL, detail: "" }];
  const decision = decideApplyQualification(report);
  assert.equal(decision.apply_qualified, false);
  assert.ok(decision.blockers.some((item) => item.includes("NTFS")));
});

test("macOS-style report cannot unlock Windows Apply", () => {
  const report = passingReport({ hostIsWindows: false, nativeRuntime: "NOT TESTED" });
  const decision = decideApplyQualification(report);
  assert.equal(decision.apply_qualified, false);
  assert.ok(decision.blockers.some((item) => item.includes("not Windows")));
});
