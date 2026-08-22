import type { FolderAccessProbe, RegisterUserContentRootResult } from "./types";
import {
  PREVIEW_CATEGORY_ORDER,
  type CategoryCounts,
} from "./oneClickSummary";

export type OneClickFolderPhase =
  | "pending"
  | "scanning"
  | "ready"
  | "authorization"
  | "denied"
  | "missing"
  | "locked"
  | "unavailable"
  | "error";

export type OneClickFolderStatus = {
  kind: string;
  label: string;
  phase: OneClickFolderPhase;
  filesIndexed?: number;
  humanStatus?: string;
};

export type OneClickScanViewProps = {
  folders: OneClickFolderStatus[];
  filesAnalyzed: number;
  progress?: string | null;
  accessSummary?: RegisterUserContentRootResult[] | null;
  onAuthorize?: () => void;
};

export function OneClickScanView({
  folders,
  filesAnalyzed,
  progress,
}: OneClickScanViewProps) {
  return (
    <section className="one-click-panel" aria-labelledby="one-click-scan-title">
      <h2 id="one-click-scan-title">Analyse de vos fichiers…</h2>
      {progress ? <p className="one-click-progress">{progress}</p> : null}
      <p className="one-click-count" aria-live="polite">
        {filesAnalyzed.toLocaleString()} fichiers analysés
      </p>
      <ul className="one-click-folder-list">
        {folders.map((folder) => (
          <li key={folder.kind}>
            <span>{folder.label}</span>
            <span className={`one-click-folder-status one-click-folder-status--${folder.phase}`}>
              {folderStatusLabel(folder)}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function needsUserGrant(state?: string | null): boolean {
  return [
    "authorization_required",
    "permission_denied",
    "unexpected_error",
  ].includes(state ?? "");
}

export type OneClickAccessViewProps = {
  folders: OneClickFolderStatus[];
  probes?: FolderAccessProbe[] | null;
  busy: boolean;
  onAuthorize: () => void;
  onRetry: () => void;
  onChooseAnother: () => void;
};

export function OneClickAccessView({
  folders,
  probes,
  busy,
  onAuthorize,
  onRetry,
  onChooseAnother,
}: OneClickAccessViewProps) {
  // FolderAccessProbe is the authoritative result of the latest native probe.
  // `folders` is UI/progress state and can briefly lag after an NSOpenPanel grant.
  // Using the stale progress state here caused the exact contradictory UI where
  // every row displayed "✓" while the heading still claimed 5 grants were needed.
  const probeStates = (probes ?? []).map((probe) => probe.accessState);
  const hasProbeState = probeStates.length > 0;
  const needsGrant = hasProbeState
    ? probeStates.some((state) => needsUserGrant(state))
    : folders.some(
        (folder) =>
          folder.phase === "authorization" ||
          folder.phase === "denied" ||
          folder.phase === "error",
      );
  const denied = hasProbeState
    ? probeStates.some((state) => state === "permission_denied")
    : folders.some((folder) => folder.phase === "denied");
  const authCount = hasProbeState
    ? probeStates.filter((state) => needsUserGrant(state)).length
    : folders.filter(
        (folder) =>
          folder.phase === "authorization" ||
          folder.phase === "denied" ||
          folder.phase === "error",
      ).length;
  const accessibleCount = hasProbeState
    ? probeStates.filter((state) => state === "accessible").length
    : 0;
  const accessResolved = hasProbeState && authCount === 0 && accessibleCount > 0;

  return (
    <section className="one-click-panel" aria-labelledby="one-click-access-title">
      <h2 id="one-click-access-title">
        {accessResolved
          ? "Accès autorisé."
          : denied
            ? "ZEMO n’a pas accès à ce dossier."
            : "ZEMO a besoin de votre autorisation pour accéder à ce dossier."}
      </h2>
      {accessResolved ? (
        <p className="one-click-note" role="status">
          ZEMO peut maintenant analyser vos dossiers personnels.
        </p>
      ) : authCount > 0 ? (
        <p className="one-click-note" role="status">
          {authCount === 1
            ? "1 dossier nécessite votre autorisation."
            : `${authCount} dossiers nécessitent votre autorisation.`}
        </p>
      ) : null}
      <ul className="one-click-folder-list">
        {(probes ?? []).length > 0
          ? probes!.map((probe) => (
              <li key={probe.kind}>
                <span>{probe.humanStatus}</span>
              </li>
            ))
          : folders.map((folder) => (
              <li key={folder.kind}>
                <span>{folder.humanStatus ?? folderStatusLabel(folder)}</span>
              </li>
            ))}
      </ul>
      <div className="one-click-actions">
        {needsGrant ? (
          <button className="primary" type="button" disabled={busy} onClick={onAuthorize}>
            {busy ? "Autorisation…" : "Autoriser l’accès"}
          </button>
        ) : accessResolved ? (
          <button className="primary" type="button" disabled={busy} onClick={onRetry}>
            {busy ? "Préparation…" : "Continuer"}
          </button>
        ) : null}
        {denied ? (
          <>
            <button className="primary" type="button" disabled={busy} onClick={onChooseAnother}>
              Choisir un autre dossier
            </button>
            <button type="button" disabled={busy} onClick={onRetry}>
              Réessayer
            </button>
          </>
        ) : !accessResolved ? (
          <button type="button" disabled={busy} onClick={onRetry}>
            Réessayer
          </button>
        ) : null}
      </div>
      {probes && probes.length > 0 ? (
        <details className="one-click-technical">
          <summary>Détails techniques</summary>
          {probes.map((probe) => (
            <pre key={probe.kind}>
              {probe.technicalDetails ??
                [
                  `Folder: ${probe.displayLabel}`,
                  `Path: ${probe.resolvedPath}`,
                  `Canonical: ${probe.canonicalPath ?? ""}`,
                  `Stage: ${probe.failedStage ?? "ok"}`,
                  `errno: ${probe.rawOsError ?? "none"}`,
                  `ErrorKind: ${probe.errorKind ?? "none"}`,
                  `PlatformError: ${probe.platformError ?? "none"}`,
                  `Inspect: ${probe.inspectResult ?? "none"}`,
                  `AccessState: ${probe.accessState}`,
                ].join("\n")}
            </pre>
          ))}
        </details>
      ) : null}
    </section>
  );
}

export type OneClickPreviewViewProps = {
  filesToOrganize: number;
  counts: CategoryCounts;
  applyBusy: boolean;
  applyEnabled: boolean;
  applyGateReason?: string | null;
  authorizationCount?: number;
  denied?: boolean;
  onApply: () => void;
  onSeeDetails: () => void;
  onAuthorize?: () => void;
  onRetry?: () => void;
  onChooseAnother?: () => void;
};

export function OneClickPreviewView({
  filesToOrganize,
  counts,
  applyBusy,
  applyEnabled,
  applyGateReason,
  authorizationCount = 0,
  denied = false,
  onApply,
  onSeeDetails,
  onAuthorize,
  onRetry,
  onChooseAnother,
}: OneClickPreviewViewProps) {
  return (
    <section className="one-click-panel" aria-labelledby="one-click-preview-title">
      <h2 id="one-click-preview-title">
        ZEMO peut ranger {filesToOrganize.toLocaleString()} fichier
        {filesToOrganize === 1 ? "" : "s"}.
      </h2>
      <ul className="one-click-category-list">
        {PREVIEW_CATEGORY_ORDER.map((category) => (
          <li key={category}>
            <span>{category}</span>
            <strong>{counts[category].toLocaleString()}</strong>
          </li>
        ))}
      </ul>
      {authorizationCount > 0 ? (
        <div className="one-click-local-error" role="status">
          <p>
            {authorizationCount === 1
              ? "1 dossier nécessite votre autorisation."
              : `${authorizationCount} dossiers nécessitent votre autorisation.`}
          </p>
          {denied ? (
            <p>ZEMO n’a pas accès à ce dossier.</p>
          ) : (
            <p>ZEMO a besoin d’accéder à vos fichiers pour les ranger.</p>
          )}
          <div className="one-click-actions">
            {denied ? (
              <>
                {onChooseAnother ? (
                  <button type="button" onClick={onChooseAnother}>
                    Choisir un autre dossier
                  </button>
                ) : null}
                {onRetry ? (
                  <button type="button" onClick={onRetry}>
                    Réessayer
                  </button>
                ) : null}
              </>
            ) : onAuthorize ? (
              <button type="button" onClick={onAuthorize}>
                Autoriser
              </button>
            ) : null}
          </div>
        </div>
      ) : null}
      {!applyEnabled ? (
        <p className="one-click-note" role="status">
          {applyGateReason ??
            "Le déplacement réel n’est pas disponible dans cette version. L’aperçu reste consultable."}
        </p>
      ) : null}
      <div className="one-click-actions">
        <button
          className="primary"
          type="button"
          disabled={!applyEnabled || applyBusy || filesToOrganize === 0}
          onClick={onApply}
        >
          {applyBusy ? "Rangement…" : "Appliquer le rangement"}
        </button>
        <button type="button" onClick={onSeeDetails}>
          Voir les détails
        </button>
      </div>
    </section>
  );
}

export type OneClickDoneViewProps = {
  filesMoved: number;
  undoBusy: boolean;
  onUndo: () => void;
  onFinish: () => void;
};

export function OneClickDoneView({
  filesMoved,
  undoBusy,
  onUndo,
  onFinish,
}: OneClickDoneViewProps) {
  return (
    <section className="one-click-panel" aria-labelledby="one-click-done-title">
      <h2 id="one-click-done-title">Rangement appliqué.</h2>
      <p>
        {filesMoved.toLocaleString()} fichier{filesMoved === 1 ? "" : "s"} rangé
        {filesMoved === 1 ? "" : "s"}
      </p>
      <p>0 fichier supprimé</p>
      <p>0 fichier écrasé</p>
      <p className="one-click-note">Les applications, raccourcis et dossiers existants protégés sont laissés en place.</p>
      <div className="one-click-actions">
        <button type="button" disabled={undoBusy} onClick={onUndo}>
          {undoBusy ? "Annulation…" : "Annuler le rangement"}
        </button>
        <button className="primary" type="button" onClick={onFinish}>
          Terminé
        </button>
      </div>
    </section>
  );
}

export function folderPhaseFromAccess(state: string | undefined): OneClickFolderPhase {
  switch (state) {
    case "accessible":
      return "pending";
    case "authorization_required":
    case "unexpected_error":
      return "authorization";
    case "permission_denied":
      return "denied";
    case "missing":
    case "unsupported":
      return "missing";
    case "locked":
      return "locked";
    case "temporarily_unavailable":
      return "unavailable";
    default:
      return state ? "error" : "pending";
  }
}

export function folderStatusFromProbe(probe: FolderAccessProbe): OneClickFolderStatus {
  return {
    kind: probe.kind,
    label: probe.displayLabel,
    phase: folderPhaseFromAccess(probe.accessState),
    humanStatus: probe.humanStatus,
  };
}

function folderStatusLabel(folder: OneClickFolderStatus): string {
  switch (folder.phase) {
    case "scanning":
      return "Analyse…";
    case "ready":
      return folder.filesIndexed != null
        ? `${folder.filesIndexed.toLocaleString()} fichiers`
        : "Prêt";
    case "authorization":
      return "Autorisation nécessaire";
    case "denied":
      return "Accès refusé";
    case "missing":
      return "Indisponible";
    case "locked":
      return "Utilisé par une autre application";
    case "unavailable":
      return "Pas disponible localement";
    case "error":
      return "Impossible à analyser";
    default:
      return "En attente";
  }
}
