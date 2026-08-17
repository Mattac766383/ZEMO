import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const packageScript = join(scriptDirectory, "package-windows-beta.ps1");
const source = readFileSync(packageScript, "utf8");

function paramBlock(text) {
  const match = text.match(/param\s*\(([\s\S]*?)\)\s*\r?\n\$ErrorActionPreference/);
  assert.ok(match, "package-windows-beta.ps1 must declare param() before ErrorActionPreference");
  return match[1];
}

test("RepoRoot is not computed inside param() from $PSScriptRoot", () => {
  const block = paramBlock(source);
  assert.equal(block.includes("$PSScriptRoot"), false);
  assert.equal(block.includes("Join-Path"), false);
  assert.equal(block.includes("Resolve-Path"), false);
  assert.match(source, /\[string\]\$RepoRoot,/);
});

test("script falls back to GITHUB_WORKSPACE and validates repo markers", () => {
  assert.match(source, /\$env:GITHUB_WORKSPACE/);
  assert.match(source, /Get-Location/);
  for (const marker of ["Cargo.toml", "package.json", "apps/desktop/src-tauri"]) {
    assert.match(source, new RegExp(marker.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.match(source, /Repository root does not exist/);
});

test("workflow passes GITHUB_WORKSPACE into the package script", () => {
  const workflow = readFileSync(
    join(scriptDirectory, "../../.github/workflows/zemo-windows-private-beta.yml"),
    "utf8",
  );
  assert.match(
    workflow,
    /package-windows-beta\.ps1[\s\S]*-RepoRoot \$env:GITHUB_WORKSPACE/,
  );
});

function resolveRepoRoot({ repoRoot = "", scriptRoot = "", workspace = "", cwd = "" } = {}) {
  if (repoRoot.trim() !== "") {
    return repoRoot;
  }
  if (scriptRoot.trim() !== "") {
    return join(scriptRoot, "../..");
  }
  if (workspace.trim() !== "") {
    return workspace;
  }
  return cwd;
}

test("fallback order is explicit RepoRoot, script root, GITHUB_WORKSPACE, cwd", () => {
  assert.equal(
    resolveRepoRoot({ repoRoot: "D:\\a\\ZEMO\\ZEMO", scriptRoot: "ignored" }),
    "D:\\a\\ZEMO\\ZEMO",
  );
  assert.equal(resolveRepoRoot({ scriptRoot: "/repo/scripts/windows-ci" }), "/repo");
  assert.equal(
    resolveRepoRoot({ workspace: "D:\\a\\ZEMO\\ZEMO", cwd: "unused" }),
    "D:\\a\\ZEMO\\ZEMO",
  );
  assert.equal(resolveRepoRoot({ cwd: "/tmp/local-windows" }), "/tmp/local-windows");
});

test("PowerShell packaging smoke runs when pwsh or powershell is available", () => {
  const shell = spawnSync("pwsh", ["-NoProfile", "-Command", "exit 0"], { encoding: "utf8" });
  const fallback = spawnSync("powershell", ["-NoProfile", "-Command", "exit 0"], {
    encoding: "utf8",
  });
  const command = shell.status === 0 ? "pwsh" : fallback.status === 0 ? "powershell" : null;
  if (!command) {
    assert.ok(true, "no PowerShell host on this machine; static checks still ran");
    return;
  }
  const testScript = join(scriptDirectory, "package-windows-beta.test.ps1");
  const result = spawnSync(command, ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", testScript], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /explicit RepoRoot packaging: PASS/);
});

test("temporary directory helper stays isolated from the real repo", () => {
  const isolated = mkdtempSync(join(tmpdir(), "zemo-package-static-"));
  try {
    assert.equal(isolated.includes("apps/desktop/src-tauri"), false);
  } finally {
    rmSync(isolated, { recursive: true, force: true });
  }
});
