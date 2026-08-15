import assert from "node:assert/strict";
import { test } from "node:test";
import { extractDiagnostics, formatCapturedFailure, linkerLines } from "./capture.mjs";

test("keeps complete LNK2005 and LNK1169 lines", () => {
  const output = [
    "libssl.lib(ssl_lib.obj) : error LNK2005: OPENSSL_init_ssl already defined in libcrypto.lib(init.obj)",
    "fatal error LNK1169: one or more multiply defined symbols found",
    "unrelated info",
  ].join("\n");
  const lines = linkerLines(output);
  assert.equal(lines.length, 2);
  assert.match(lines[0], /LNK2005/);
  assert.match(lines[0], /OPENSSL_init_ssl/);
  assert.match(lines[0], /libssl\.lib/);
  assert.match(lines[0], /libcrypto\.lib/);
  assert.match(lines[1], /LNK1169/);
});

test("extracts failed test names, panics, win32, stages, and DLLs", () => {
  const output = [
    "test sharing_violation_is_retryable ... FAILED",
    "test standard_move ... ok",
    "thread 'sharing_violation' panicked at crates/x.rs:1:1:",
    "os error 87",
    "ERROR_INVALID_PARAMETER",
    "could not load onnxruntime.dll",
    "STAGE ORT LOAD: FAIL: missing onnxruntime.dll",
    "STAGE GRANITE EMBEDDING: PASS",
  ].join("\n");
  const captured = extractDiagnostics(output);
  assert.deepEqual(captured.failedTests, ["sharing_violation_is_retryable"]);
  assert.equal(captured.panics.length, 1);
  assert.equal(captured.win32.length, 2);
  assert.ok(captured.dlls.some((line) => line.includes("onnxruntime.dll")));
  assert.deepEqual(
    captured.stages.map((stage) => `${stage.name}:${stage.status}`),
    ["ORT LOAD:FAIL", "GRANITE EMBEDDING:PASS"],
  );
});

test("failure formatter does not drop the only LNK line", () => {
  const formatted = formatCapturedFailure({
    command: "cargo test -p application --no-run",
    exitCode: 101,
    output: "fatal error LNK1169: one or more multiply defined symbols found\n",
    logPath: "target/windows-qualification/logs/link.log",
  });
  assert.match(formatted, /command: cargo test/);
  assert.match(formatted, /exit: 101/);
  assert.match(formatted, /LNK1169/);
  assert.match(formatted, /link\.log/);
});
