/**
 * Extract complete, non-truncated failure signals from command output.
 */

const LINKER_RE =
  /LNK\d+|error LNK|linking with `link\.exe`|already defined|multiply defined|unresolved external/i;
const TEST_RE = /^test\s+(\S+)\s+\.\.\.\s+(ok|FAILED|ignored)/;
const PANIC_RE = /panicked at|thread '.*' panicked|assertion `.*` failed/i;
const RUST_ERROR_RE = /error\[E\d+\]|error: /;
const WIN32_RE =
  /os error\s+\d+|ERROR_\w+|GetLastError|ERROR_INVALID_PARAMETER|The parameter is incorrect/i;
const DLL_RE = /\.dll|onnxruntime|usearch|not found|cannot find|delayload/i;

export function linkerLines(output) {
  return String(output || "")
    .split(/\r?\n/)
    .filter((line) => /LNK\d+|error LNK/i.test(line) || LINKER_RE.test(line));
}

export function extractDiagnostics(output) {
  const lines = String(output || "").split(/\r?\n/);
  const tests = [];
  const failedTests = [];
  const panics = [];
  const rustErrors = [];
  const win32 = [];
  const dlls = [];
  const stages = [];

  for (const line of lines) {
    const testMatch = line.match(TEST_RE);
    if (testMatch) {
      const entry = { name: testMatch[1], result: testMatch[2] };
      tests.push(entry);
      if (entry.result === "FAILED") {
        failedTests.push(entry.name);
      }
    }
    if (PANIC_RE.test(line)) {
      panics.push(line);
    }
    if (RUST_ERROR_RE.test(line)) {
      rustErrors.push(line);
    }
    if (WIN32_RE.test(line)) {
      win32.push(line);
    }
    if (DLL_RE.test(line) && /dll|onnxruntime|usearch|not found/i.test(line)) {
      dlls.push(line);
    }
    const stage = line.match(/^STAGE\s+(.+):\s+(PASS|FAIL)(?::\s*(.*))?$/);
    if (stage) {
      stages.push({
        name: stage[1],
        status: stage[2],
        detail: stage[3] || "",
      });
    }
  }

  return {
    tests,
    failedTests,
    panics,
    rustErrors,
    win32,
    dlls,
    stages,
    linker: linkerLines(output),
  };
}

export function formatCapturedFailure({ command, exitCode, output, logPath }) {
  const captured = extractDiagnostics(output);
  const lines = String(output || "").split(/\r?\n/);
  const parts = [
    `command: ${command}`,
    `exit: ${exitCode}`,
  ];
  if (logPath) {
    parts.push(`log: ${logPath}`);
  }
  if (captured.failedTests.length) {
    parts.push(`failed tests: ${captured.failedTests.join(", ")}`);
  }
  if (captured.linker.length) {
    parts.push("LNKxxxx:");
    parts.push(captured.linker.join("\n"));
  }
  if (captured.panics.length) {
    parts.push("panics:");
    parts.push(captured.panics.join("\n"));
  }
  if (captured.rustErrors.length) {
    parts.push("rust errors:");
    parts.push(captured.rustErrors.slice(-30).join("\n"));
  }
  if (captured.win32.length) {
    parts.push("win32:");
    parts.push(captured.win32.join("\n"));
  }
  if (captured.dlls.length) {
    parts.push("dll/native:");
    parts.push(captured.dlls.slice(-20).join("\n"));
  }
  if (captured.stages.length) {
    parts.push("stages:");
    parts.push(
      captured.stages
        .map((stage) => `${stage.name}: ${stage.status}${stage.detail ? ` ${stage.detail}` : ""}`)
        .join("\n"),
    );
  }
  parts.push("--- tail ---");
  parts.push(lines.slice(-80).join("\n"));
  return parts.join("\n");
}
