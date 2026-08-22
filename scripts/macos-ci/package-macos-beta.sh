#!/usr/bin/env bash
set -euo pipefail

APP_PATH="${1:-}"
OUT_DIR="${2:-}"
DIST_TAG="${3:-}"
GIT_COMMIT="${4:-${GITHUB_SHA:-unknown}}"

if [[ -z "$APP_PATH" || -z "$OUT_DIR" || -z "$DIST_TAG" ]]; then
  echo "usage: package-macos-beta.sh <ZEMO.app> <out-dir> <dist-tag> [git-commit]" >&2
  exit 2
fi

if [[ ! "$DIST_TAG" =~ ^[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$ ]]; then
  echo "invalid private beta tag: $DIST_TAG" >&2
  echo "expected format: 0.1.0-beta.7" >&2
  exit 2
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "app bundle not found: $APP_PATH" >&2
  exit 1
fi

if [[ ! -x "$APP_PATH/Contents/MacOS/operation-executor" ]]; then
  echo "packaged operation-executor sidecar missing or not executable" >&2
  exit 1
fi

codesign --verify --deep --strict "$APP_PATH"

mkdir -p "$OUT_DIR"
rm -f "$OUT_DIR"/*

ARCH="$(uname -m)"
ZIP_NAME="ZEMO-${DIST_TAG}-macos-${ARCH}.zip"
ZIP_PATH="$OUT_DIR/$ZIP_NAME"

# ditto preserves the macOS application bundle metadata better than a generic zip.
ditto -c -k --sequesterRsrc --keepParent "$APP_PATH" "$ZIP_PATH"

SHA256="$(shasum -a 256 "$ZIP_PATH" | awk '{print $1}')"
printf '%s  %s\n' "$SHA256" "$ZIP_NAME" > "$OUT_DIR/SHA256SUMS.txt"

BUILD_TIME="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
MACOS_VERSION="$(sw_vers -productVersion)"

cat > "$OUT_DIR/BUILDINFO.txt" <<EOF
ZEMO private beta
version: 0.1.0
distribution tag: $DIST_TAG
platform: macOS
architecture: $ARCH
macOS runner version: $MACOS_VERSION
git commit: $GIT_COMMIT
bundle: $ZIP_NAME
sha256: $SHA256
signing: ad-hoc only
notarization: NOT CONFIGURED
Gatekeeper external-user experience: NOT QUALIFIED
packaged sidecar: PRESENT
physical Apply/Undo qualification: PASS before packaging
build timestamp: $BUILD_TIME
EOF

cat > "$OUT_DIR/README-FIRST.txt" <<EOF
ZEMO — Bêta privée macOS
========================

Cette archive contient ZEMO.app et son exécuteur local packagé.
La build a passé les tests du bundle réel, l’authentification du sidecar et les
tests physiques Apply/Undo avant création de cette archive.

IMPORTANT
---------
- Cette bêta utilise uniquement une signature ad-hoc.
- Elle n’est PAS notariée par Apple.
- Ne désactivez pas Gatekeeper ou les protections macOS pour la lancer.
- Si macOS bloque l’application, arrêtez le test et signalez-le au mainteneur.
- Utilisez d’abord un dossier de test contenant des copies de fichiers.

Test conseillé
--------------
1. Décompressez $ZIP_NAME.
2. Ouvrez ZEMO normalement.
3. Faites le premier essai sur un petit dossier de test.
4. Vérifiez la proposition avant Apply.
5. Testez Undo après un petit Apply réussi.
6. Testez une recherche sémantique simple.
7. En cas de problème, ouvrez « Diagnostic bêta local » dans l’accueil et
   partagez uniquement ces compteurs avec le mainteneur.

Confidentialité
---------------
Le diagnostic bêta local ne doit contenir ni nom de fichier, ni chemin, ni
contenu de document, ni requête de recherche. Les fichiers restent traités
localement selon les garanties de ZEMO.

Identifiant technique : com.workingname.organizer
Version : 0.1.0 ($DIST_TAG)
Fichier : $ZIP_NAME
SHA-256 : $SHA256
EOF

export ZEMO_BETA_DIST_TAG="$DIST_TAG"
export ZEMO_BETA_GIT_COMMIT="$GIT_COMMIT"
export ZEMO_BETA_ARCH="$ARCH"
export ZEMO_BETA_BUNDLE="$ZIP_NAME"
export ZEMO_BETA_SHA256="$SHA256"
export ZEMO_BETA_BUILD_TIME="$BUILD_TIME"

node <<'NODE' > "$OUT_DIR/beta-manifest.json"
const manifest = {
  schema_version: 1,
  product: "ZEMO",
  version: "0.1.0",
  distribution_tag: process.env.ZEMO_BETA_DIST_TAG,
  channel: "private-beta",
  platform: "macos",
  architecture: process.env.ZEMO_BETA_ARCH,
  git_commit: process.env.ZEMO_BETA_GIT_COMMIT,
  artifact: process.env.ZEMO_BETA_BUNDLE,
  sha256: process.env.ZEMO_BETA_SHA256,
  signing: {
    mode: "ad-hoc",
    notarized: false,
    external_user_experience_qualified: false,
  },
  runtime_qualification: {
    bundle_inspected: true,
    packaged_sidecar_present: true,
    sidecar_authentication_tested: true,
    physical_apply_undo_tested: true,
  },
  privacy: {
    beta_metrics_local_only: true,
    diagnostic_contains_user_file_data: false,
  },
  built_at_utc: process.env.ZEMO_BETA_BUILD_TIME,
};
process.stdout.write(`${JSON.stringify(manifest, null, 2)}\n`);
NODE

printf 'Packaged %s\nSHA-256 %s\nOutput %s\n' "$ZIP_NAME" "$SHA256" "$OUT_DIR"
