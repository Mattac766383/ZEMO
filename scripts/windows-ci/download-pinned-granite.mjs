#!/usr/bin/env node
/**
 * Download the pinned public Granite embedding assets from the production
 * manifest. Verifies SHA-256 before exposing the directory to tests.
 *
 * Testers still do not need SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR.
 * This script is CI-only so SEMANTIC qualification can run on the runner.
 */

import { createHash } from "node:crypto";
import {
  createReadStream,
  createWriteStream,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryDirectory = resolve(scriptDirectory, "../..");
const manifestPath = join(
  repositoryDirectory,
  "models/manifests/granite-embedding-97m-multilingual-r2.v1.json",
);
const PINNED_HOST = "huggingface.co";
const PINNED_REPO = "ibm-granite/granite-embedding-97m-multilingual-r2";

const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const destRoot =
  process.env.ZEMO_PINNED_MODEL_CACHE ||
  join(repositoryDirectory, "target", "windows-ci-models", manifest.model_id, manifest.revision);
const statusPath = join(destRoot, "granite-status.json");

function record(status, detail) {
  mkdirSync(destRoot, { recursive: true });
  const payload = {
    status,
    detail,
    model_id: manifest.model_id,
    revision: manifest.revision,
    directory: destRoot,
  };
  writeFileSync(statusPath, `${JSON.stringify(payload, null, 2)}\n`);
  appendGithubFile(process.env.GITHUB_OUTPUT, [
    `status=${status}`,
    `directory=${destRoot}`,
  ]);
  if (status === "PASS") {
    appendGithubFile(process.env.GITHUB_ENV, [
      `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR=${destRoot}`,
      `ZEMO_GRANITE_STATUS=${status}`,
    ]);
  } else {
    appendGithubFile(process.env.GITHUB_ENV, [`ZEMO_GRANITE_STATUS=${status}`]);
  }
  process.stdout.write(`Granite pinned download: ${status} — ${detail}\n`);
  process.stdout.write(`Directory: ${destRoot}\n`);
}

function appendGithubFile(filePath, lines) {
  if (!filePath) {
    return;
  }
  writeFileSync(filePath, `${lines.join("\n")}\n`, { flag: "a" });
}

function pinnedAssetUrl(revision, assetPath) {
  return `https://${PINNED_HOST}/${PINNED_REPO}/resolve/${revision}/${assetPath}?download=true`;
}

function sha256File(filePath) {
  return new Promise((resolveHash, reject) => {
    const hash = createHash("sha256");
    const stream = createReadStream(filePath);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", () => resolveHash(hash.digest("hex")));
  });
}

async function assetMatches(dest, asset) {
  if (!existsSync(dest)) {
    return false;
  }
  const stat = statSync(dest);
  if (Number(stat.size) !== Number(asset.bytes)) {
    return false;
  }
  const digest = await sha256File(dest);
  return digest === String(asset.sha256).toLowerCase();
}

async function downloadToFile(url, dest) {
  const response = await fetch(url, {
    redirect: "follow",
    headers: {
      "User-Agent": "ZEMO-windows-ci/1.0",
      Accept: "application/octet-stream",
    },
  });
  if (!response.ok || !response.body) {
    throw new Error(`download failed ${response.status} for ${url}`);
  }
  const tmp = `${dest}.partial`;
  rmSync(tmp, { force: true });
  await pipeline(Readable.fromWeb(response.body), createWriteStream(tmp));
  renameSync(tmp, dest);
}

async function main() {
  mkdirSync(destRoot, { recursive: true });
  try {
    for (const asset of manifest.assets) {
      const dest = join(destRoot, asset.path);
      mkdirSync(dirname(dest), { recursive: true });
      if (await assetMatches(dest, asset)) {
        process.stdout.write(`Cached and verified: ${asset.path}\n`);
        continue;
      }
      const url = pinnedAssetUrl(manifest.revision, asset.path);
      process.stdout.write(`Downloading ${asset.path} (${asset.bytes} bytes)\n`);
      rmSync(dest, { force: true });
      await downloadToFile(url, dest);
      if (!(await assetMatches(dest, asset))) {
        rmSync(dest, { force: true });
        throw new Error(`SHA-256 mismatch after download: ${asset.path}`);
      }
      process.stdout.write(`Verified: ${asset.path}\n`);
    }
    record("PASS", "pinned assets downloaded and SHA-256 verified");
  } catch (error) {
    record("FAIL", String(error?.message || error));
    process.exitCode = 1;
  }
}

await main();
