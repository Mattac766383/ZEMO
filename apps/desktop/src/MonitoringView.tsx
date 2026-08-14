import {
  type FormEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import {
  addMonitoringExclusion,
  cancelMonitoring,
  getMonitoringDashboard,
  pauseMonitoring,
  removeMonitoringExclusion,
  resumeMonitoring,
  runMonitoringCycle,
  setMonitoredFolderEnabled,
} from "./api";
import { classifyUserError } from "./errors";
import type {
  MonitoredFolder,
  MonitoringActivity,
  MonitoringDashboard,
  MonitoringExclusion,
} from "./types";

export interface MonitoringViewProps {
  workspaceId: string;
  onOpenReview?: () => void;
  onOpenOrganization?: (rootId: string) => void;
}

interface DashboardRequest {
  workspaceId: string;
  promise: Promise<MonitoringDashboard | null>;
}

type ExclusionKind = MonitoringExclusion["kind"];

export function MonitoringView({
  workspaceId,
  onOpenReview,
  onOpenOrganization,
}: MonitoringViewProps) {
  const [dashboard, setDashboard] = useState<MonitoringDashboard | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exclusionKind, setExclusionKind] =
    useState<ExclusionKind>("path_prefix");
  const [exclusionValue, setExclusionValue] = useState("");
  const [exclusionRootId, setExclusionRootId] = useState("");
  const [exclusionError, setExclusionError] = useState<string | null>(null);

  const mountedRef = useRef(false);
  const busyRef = useRef(false);
  const workspaceRef = useRef(workspaceId);
  const dashboardRequestRef = useRef<DashboardRequest | null>(null);
  workspaceRef.current = workspaceId;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const refreshDashboard = useCallback(
    (surfaceErrors = true): Promise<MonitoringDashboard | null> => {
      const currentRequest = dashboardRequestRef.current;
      if (currentRequest?.workspaceId === workspaceId) {
        return currentRequest.promise;
      }

      let request!: Promise<MonitoringDashboard | null>;
      request = getMonitoringDashboard(workspaceId)
        .then((nextDashboard) => {
          if (
            mountedRef.current &&
            workspaceRef.current === workspaceId
          ) {
            setDashboard(nextDashboard);
            setError(null);
          }
          return nextDashboard;
        })
        .catch((reason: unknown) => {
          if (
            surfaceErrors &&
            mountedRef.current &&
            workspaceRef.current === workspaceId
          ) {
            setError(classifyUserError(reason, "monitoring").message);
          }
          return null;
        })
        .finally(() => {
          if (dashboardRequestRef.current?.promise === request) {
            dashboardRequestRef.current = null;
          }
        });

      dashboardRequestRef.current = { workspaceId, promise: request };
      return request;
    },
    [workspaceId],
  );

  useEffect(() => {
    setDashboard((current) =>
      current?.workspaceId === workspaceId ? current : null,
    );
    setError(null);
    setBusy(null);
    setCancelling(false);
    busyRef.current = false;
    void refreshDashboard(true);

    const interval = window.setInterval(() => {
      if (!busyRef.current) {
        void refreshDashboard(false);
      }
    }, 3_000);

    return () => {
      window.clearInterval(interval);
    };
  }, [refreshDashboard, workspaceId]);

  async function performAction(
    action: string,
    operation: () => Promise<void | MonitoringDashboard>,
  ): Promise<boolean> {
    if (busyRef.current) {
      return false;
    }

    const actionWorkspace = workspaceId;
    busyRef.current = true;
    setBusy(action);
    setError(null);

    try {
      const pendingDashboard = dashboardRequestRef.current;
      if (pendingDashboard?.workspaceId === actionWorkspace) {
        await pendingDashboard.promise;
      }
      if (
        !mountedRef.current ||
        workspaceRef.current !== actionWorkspace
      ) {
        return false;
      }

      const result = await operation();
      if (
        !mountedRef.current ||
        workspaceRef.current !== actionWorkspace
      ) {
        return false;
      }

      if (isMonitoringDashboard(result)) {
        setDashboard(result);
        setError(null);
      } else {
        await refreshDashboard(true);
      }
      return true;
    } catch (reason) {
      if (
        mountedRef.current &&
        workspaceRef.current === actionWorkspace
      ) {
        setError(classifyUserError(reason, "monitoring").message);
      }
      return false;
    } finally {
      if (
        mountedRef.current &&
        workspaceRef.current === actionWorkspace
      ) {
        busyRef.current = false;
        setBusy(null);
      }
    }
  }

  async function submitExclusion(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const sanitized = sanitizeExclusion(exclusionKind, exclusionValue);
    if ("error" in sanitized) {
      setExclusionError(sanitized.error);
      return;
    }

    setExclusionError(null);
    const added = await performAction("add-exclusion", () =>
      addMonitoringExclusion(
        workspaceId,
        exclusionRootId || undefined,
        exclusionKind,
        sanitized.value,
      ),
    );
    if (added && mountedRef.current) {
      setExclusionValue("");
    }
  }

  async function cancelCurrentCheck() {
    if (cancelling) {
      return;
    }
    const actionWorkspace = workspaceId;
    setCancelling(true);
    setError(null);
    try {
      await cancelMonitoring(actionWorkspace);
      if (
        mountedRef.current &&
        workspaceRef.current === actionWorkspace &&
        !busyRef.current
      ) {
        await refreshDashboard(false);
      }
    } catch (reason) {
      if (
        mountedRef.current &&
        workspaceRef.current === actionWorkspace
      ) {
        setError(classifyUserError(reason, "monitoring").message);
      }
    } finally {
      if (
        mountedRef.current &&
        workspaceRef.current === actionWorkspace
      ) {
        setCancelling(false);
      }
    }
  }

  const visibleDashboard =
    dashboard?.workspaceId === workspaceId ? dashboard : null;
  const safetyInvariantFailed =
    visibleDashboard?.automaticExecutionEnabled !== undefined &&
    visibleDashboard.automaticExecutionEnabled !== false;

  return (
    <section
      className="monitoring-view"
      aria-labelledby="monitoring-title"
      aria-busy={busy !== null}
    >
      <header className="monitoring-header">
        <div>
          <span className="eyebrow">Surveillance locale</span>
          <h2 id="monitoring-title">Surveillance</h2>
          <p>
            Détecte les nouveaux fichiers dans les dossiers choisis et prépare
            de nouvelles propositions. Les fichiers ne sont pas déplacés
            automatiquement.
          </p>
        </div>
        {visibleDashboard ? (
          <div className="monitoring-state" aria-label="État de la surveillance">
            <span className={workspaceHealthTone(visibleDashboard)}>
              {workspaceHealth(visibleDashboard)}
            </span>
            <span>Local uniquement</span>
          </div>
        ) : null}
      </header>

      <div className="monitoring-safety" role="status">
        <strong>Surveillance = propositions uniquement</strong>
        <p>
          La surveillance prépare des propositions d’organisation. Elle ne
          déplace, ne renomme ni ne supprime jamais de fichiers automatiquement.
        </p>
      </div>

      {safetyInvariantFailed ? (
        <div className="error-banner" role="alert">
          Contrôle de sécurité de la surveillance échoué. L’exécution
          automatique doit rester désactivée.
        </div>
      ) : null}

      {error ? (
        <div className="notice-banner notice-banner--warning monitoring-error" role="status">
          <span>
            <strong>Surveillance interrompue</strong>
            {error}
          </span>
          <button
            type="button"
            aria-label="Fermer l’erreur de surveillance"
            onClick={() => setError(null)}
          >
            Fermer
          </button>
        </div>
      ) : null}

      {!visibleDashboard ? (
        <p className="monitoring-loading" role="status">
          Chargement de la surveillance…
        </p>
      ) : (
        <>
          {visibleDashboard.startupReconciliationPending ? (
            <div
              className="monitoring-reconciliation"
              role="status"
              aria-live="polite"
            >
              <strong>Mise à jour au démarrage en cours</strong>
              <p>
                L’inventaire local est mis à jour avant de reprendre la surveillance.
                Aucune modification de fichiers n’est effectuée.
              </p>
            </div>
          ) : null}

          <div className="monitoring-controls">
            <button
              type="button"
              disabled={busy !== null}
              onClick={() =>
                void performAction(
                  visibleDashboard.paused ? "resume" : "pause",
                  () =>
                    visibleDashboard.paused
                      ? resumeMonitoring(workspaceId)
                      : pauseMonitoring(workspaceId),
                )
              }
            >
              {busy === "pause"
                ? "Mise en pause…"
                : busy === "resume"
                  ? "Reprise…"
                  : visibleDashboard.paused
                    ? "Reprendre la surveillance"
                    : "Mettre en pause"}
            </button>
            <button
              type="button"
              className="primary-action"
              disabled={
                busy !== null ||
                visibleDashboard.startupReconciliationPending
              }
              onClick={() =>
                void performAction("run", () =>
                  runMonitoringCycle(workspaceId),
                )
              }
            >
              {busy === "run" ? "Vérification…" : "Vérifier maintenant"}
            </button>
            <button
              type="button"
              disabled={cancelling || (busy !== null && busy !== "run")}
              onClick={() => void cancelCurrentCheck()}
            >
              {cancelling ? "Annulation…" : "Annuler la vérification"}
            </button>
          </div>

          <section
            className="monitoring-panel"
            aria-labelledby="monitoring-overview-title"
          >
            <div className="monitoring-panel-heading">
              <div>
                <span className="step">Espace actuel</span>
                <h3 id="monitoring-overview-title">Vue d’ensemble</h3>
              </div>
              <div className="monitoring-navigation">
                {onOpenReview && visibleDashboard.counts.needsReview > 0 ? (
                  <button type="button" onClick={onOpenReview}>
                    Ouvrir À revoir
                  </button>
                ) : null}
                {onOpenOrganization &&
                visibleDashboard.folders.length === 1 &&
                (visibleDashboard.counts.readyToOrganize > 0 ||
                  visibleDashboard.counts.pendingProposals > 0) ? (
                  <button
                    type="button"
                    onClick={() =>
                      onOpenOrganization(visibleDashboard.folders[0].rootId)
                    }
                  >
                    Ouvrir l’organisation
                  </button>
                ) : null}
              </div>
            </div>
            <div
              className="monitoring-metrics"
              aria-label="Indicateurs de surveillance"
            >
              <MonitoringMetric
                label="Fichiers analysés"
                value={visibleDashboard.counts.filesAnalyzed}
              />
              <MonitoringMetric
                label="Prêts à organiser"
                value={visibleDashboard.counts.readyToOrganize}
                tone="ready"
              />
              <MonitoringMetric
                label="À revoir"
                value={visibleDashboard.counts.needsReview}
                tone="review"
              />
              <MonitoringMetric
                label="Propositions en attente"
                value={visibleDashboard.counts.pendingProposals}
              />
              <MonitoringMetric
                label="Tâches en attente"
                value={visibleDashboard.counts.pendingJobs}
              />
            </div>
          </section>

          <section
            className="monitoring-panel"
            aria-labelledby="monitored-folders-title"
          >
            <div className="monitoring-panel-heading">
              <div>
                <span className="step">Choisis localement</span>
                <h3 id="monitored-folders-title">Dossiers surveillés</h3>
              </div>
              <span className="monitoring-count">
                {visibleDashboard.folders.length.toLocaleString()} sélectionné
                {visibleDashboard.folders.length === 1 ? "" : "s"}
              </span>
            </div>
            {visibleDashboard.folders.length === 0 ? (
              <p className="empty-state">Aucun dossier surveillé pour le moment. Ajoutez un dossier via Fichiers, puis activez-le ici.</p>
            ) : (
              <ul className="monitoring-folder-list">
                {visibleDashboard.folders.map((folder) => (
                  <FolderCard
                    key={folder.rootId}
                    folder={folder}
                    disabled={busy !== null}
                    busy={busy === `folder:${folder.rootId}`}
                    onOpenOrganization={
                      onOpenOrganization
                        ? () => onOpenOrganization(folder.rootId)
                        : undefined
                    }
                    onToggle={() =>
                      performAction(`folder:${folder.rootId}`, () =>
                        setMonitoredFolderEnabled(
                          folder.rootId,
                          !folder.enabled,
                        ),
                      )
                    }
                  />
                ))}
              </ul>
            )}
          </section>

          <section
            className="monitoring-panel"
            aria-labelledby="recent-monitoring-activity-title"
          >
            <div className="monitoring-panel-heading">
              <div>
                <span className="step">Mises à jour</span>
                <h3 id="recent-monitoring-activity-title">Activité récente</h3>
              </div>
            </div>
            {visibleDashboard.recentActivity.length === 0 ? (
              <p className="empty-state">Pas encore d’activité. Lancez une vérification ou attendez de nouveaux fichiers.</p>
            ) : (
              <ol className="monitoring-activity-list">
                {visibleDashboard.recentActivity.map((activity) => (
                  <ActivityCard key={activity.id} activity={activity} />
                ))}
              </ol>
            )}
          </section>

          <section
            className="monitoring-panel"
            aria-labelledby="monitoring-exclusions-title"
          >
            <div className="monitoring-panel-heading">
              <div>
                <span className="step">Moins de bruit</span>
                <h3 id="monitoring-exclusions-title">Exclusions</h3>
              </div>
            </div>
            <form className="monitoring-exclusion-form" onSubmit={submitExclusion}>
              <label>
                Type d’exclusion
                <select
                  value={exclusionKind}
                  onChange={(event) => {
                    setExclusionKind(
                      event.currentTarget.value as ExclusionKind,
                    );
                    setExclusionError(null);
                  }}
                >
                  <option value="path_prefix">Préfixe de chemin</option>
                  <option value="extension">Extension de fichier</option>
                </select>
              </label>
              <label>
                Périmètre
                <select
                  value={exclusionRootId}
                  onChange={(event) =>
                    setExclusionRootId(event.currentTarget.value)
                  }
                >
                  <option value="">Tous les dossiers surveillés</option>
                  {visibleDashboard.folders.map((folder) => (
                    <option key={folder.rootId} value={folder.rootId}>
                      {folder.displayLabel}
                    </option>
                  ))}
                </select>
              </label>
              <label className="monitoring-exclusion-value">
                Valeur
                <input
                  value={exclusionValue}
                  maxLength={240}
                  placeholder={
                    exclusionKind === "extension"
                      ? ".tmp"
                      : "Cache/generated"
                  }
                  aria-describedby="monitoring-exclusion-help"
                  onChange={(event) => {
                    setExclusionValue(event.currentTarget.value);
                    setExclusionError(null);
                  }}
                />
              </label>
              <button
                type="submit"
                className="primary-action"
                disabled={busy !== null}
              >
                {busy === "add-exclusion" ? "Ajout…" : "Ajouter une exclusion"}
              </button>
            </form>
            <p id="monitoring-exclusion-help" className="monitoring-help">
              Les chemins doivent être relatifs à un dossier surveillé. Les
              extensions sont normalisées en minuscules avec un point initial.
            </p>
            {exclusionError ? (
              <p className="inline-error" role="alert">
                {exclusionError}
              </p>
            ) : null}
            {visibleDashboard.exclusions.length === 0 ? (
              <p className="empty-state">Aucune exclusion pour le moment.</p>
            ) : (
              <ul className="monitoring-exclusion-list">
                {visibleDashboard.exclusions.map((exclusion) => {
                  const folder = visibleDashboard.folders.find(
                    (candidate) =>
                      candidate.rootId === exclusion.rootId,
                  );
                  return (
                    <li key={exclusion.id}>
                      <div>
                        <span className="monitoring-exclusion-kind">
                          {exclusion.kind === "extension"
                            ? "Extension"
                            : "Préfixe de chemin"}
                        </span>
                        <code>{exclusion.value}</code>
                        <small>
                          {folder?.displayLabel ?? "Tous les dossiers surveillés"} ·{" "}
                          {exclusion.enabled ? "Activée" : "Désactivée"}
                        </small>
                      </div>
                      <button
                        type="button"
                        disabled={busy !== null}
                        aria-label={`Retirer l’exclusion ${exclusion.value}`}
                        onClick={() =>
                          void performAction(
                            `remove-exclusion:${exclusion.id}`,
                            () =>
                              removeMonitoringExclusion(exclusion.id),
                          )
                        }
                      >
                        {busy === `remove-exclusion:${exclusion.id}`
                          ? "Suppression…"
                          : "Retirer"}
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        </>
      )}
    </section>
  );
}

function MonitoringMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "ready" | "review";
}) {
  return (
    <div
      className={`monitoring-metric${tone ? ` ${tone}` : ""}`}
      aria-label={label}
    >
      <span>{label}</span>
      <strong>{value.toLocaleString()}</strong>
    </div>
  );
}

function FolderCard({
  folder,
  disabled,
  busy,
  onToggle,
  onOpenOrganization,
}: {
  folder: MonitoredFolder;
  disabled: boolean;
  busy: boolean;
  onToggle: () => Promise<boolean>;
  onOpenOrganization?: () => void;
}) {
  return (
    <li
      className={`monitoring-folder${folder.enabled ? "" : " disabled"}`}
    >
      <div className="monitoring-folder-main">
        <div>
          <strong>{folder.displayLabel}</strong>
          <code>{folder.selectedPath}</code>
        </div>
        <span
          className={`monitoring-folder-status ${statusTone(folder.status)}`}
          aria-label={`État du dossier : ${humanize(folder.status)}`}
        >
          {humanize(folder.status)}
        </span>
      </div>
      <div className="monitoring-folder-meta">
        <span>{folder.pendingJobs.toLocaleString()} tâches en attente</span>
        <span>
          Dernière mise à jour : {formatTimestamp(folder.lastReconciledAt)}
        </span>
        <span>{folder.enabled ? "Activé" : "Désactivé"}</span>
        <span>État : {healthState(folder.status, folder.enabled)}</span>
      </div>
      {folder.lastError ? (
        <p className="monitoring-folder-error">
          Dernière erreur : {classifyUserError(folder.lastError, "monitoring").message}
        </p>
      ) : null}
      <div className="monitoring-navigation">
        {onOpenOrganization ? (
          <button type="button" onClick={onOpenOrganization}>
            Voir la proposition de ce dossier
          </button>
        ) : null}
        <button
          type="button"
          disabled={disabled}
          aria-label={`${folder.enabled ? "Désactiver" : "Activer"} ${folder.displayLabel}`}
          onClick={() => void onToggle()}
        >
          {busy
            ? "Mise à jour…"
            : folder.enabled
              ? "Désactiver le dossier"
              : "Activer le dossier"}
        </button>
      </div>
    </li>
  );
}

function ActivityCard({ activity }: { activity: MonitoringActivity }) {
  return (
    <li>
      <article className="monitoring-activity">
        <div>
          <strong>{activity.summary}</strong>
          <time dateTime={activity.createdAt}>
            {formatTimestamp(activity.createdAt)}
          </time>
        </div>
        <p>{formatActivity(activity)}</p>
      </article>
    </li>
  );
}

function formatActivity(activity: MonitoringActivity): string {
  const parts = [
    `${activity.filesAnalyzed.toLocaleString()} nouveau${
      activity.filesAnalyzed === 1 ? "" : "x"
    } fichier${activity.filesAnalyzed === 1 ? "" : "s"} analysé${
      activity.filesAnalyzed === 1 ? "" : "s"
    }`,
    `${activity.readyToOrganize.toLocaleString()} prêts à organiser`,
    `${activity.needsReview.toLocaleString()} à revoir`,
  ];
  if (activity.failed > 0) {
    parts.push(`${activity.failed.toLocaleString()} en échec`);
  }
  return parts.join(" · ");
}

function formatTimestamp(value?: string | null): string {
  if (!value) {
    return "Pas encore";
  }
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Indisponible" : date.toLocaleString();
}

function humanize(value: string): string {
  return value
    .trim()
    .toLocaleLowerCase()
    .split(/[_\s-]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toLocaleUpperCase() + part.slice(1))
    .join(" ");
}

function statusTone(status: string): string {
  const normalized = status.toLocaleUpperCase();
  if (
    normalized.includes("ERROR") ||
    normalized.includes("FAIL") ||
    normalized.includes("BLOCK") ||
    normalized.includes("OVERFLOW") ||
    normalized.includes("OFFLINE") ||
    normalized.includes("STOP")
  ) {
    return "error";
  }
  if (
    normalized.includes("PENDING") ||
    normalized.includes("RECONCIL") ||
    normalized.includes("RUNNING") ||
    normalized.includes("START") ||
    normalized.includes("PAUS")
  ) {
    return "working";
  }
  return ["ACTIVE", "WATCHING"].includes(normalized) ? "ready" : "error";
}

function healthState(status: string, enabled: boolean): string {
  if (!enabled || ["PAUSED", "STOPPED"].includes(status.toLocaleUpperCase())) {
    return "En pause";
  }
  switch (status.toLocaleUpperCase()) {
    case "ACTIVE":
    case "WATCHING":
      return "Saine";
    case "OVERFLOWED":
      return "Saturée";
    case "OFFLINE":
      return "Hors ligne";
    case "FAILED":
      return "Erreur";
    default:
      return "Dégradée";
  }
}

function workspaceHealth(dashboard: MonitoringDashboard): string {
  if (dashboard.paused) {
    return "En pause";
  }
  const statuses = dashboard.folders
    .filter((folder) => folder.enabled)
    .map((folder) => folder.status.toLocaleUpperCase());
  if (statuses.some((status) => status === "FAILED")) {
    return "Erreur";
  }
  if (statuses.some((status) => status === "OVERFLOWED")) {
    return "Saturée";
  }
  if (statuses.some((status) => status === "OFFLINE")) {
    return "Hors ligne";
  }
  if (
    statuses.length > 0 &&
    statuses.every((status) => ["ACTIVE", "WATCHING"].includes(status))
  ) {
    return "Saine";
  }
  return "Dégradée";
}

function workspaceHealthTone(dashboard: MonitoringDashboard): string {
  const health = workspaceHealth(dashboard);
  if (health === "Saine") {
    return "active";
  }
  return health === "En pause" ? "paused" : "error";
}

function sanitizeExclusion(
  kind: ExclusionKind,
  rawValue: string,
): { value: string } | { error: string } {
  if (kind === "extension") {
    const withoutLeadingDots = rawValue
      .trim()
      .toLocaleLowerCase()
      .replace(/^\.+/, "");
    if (!withoutLeadingDots) {
      return { error: "Indiquez une extension de fichier à exclure." };
    }
    if (
      withoutLeadingDots.length > 32 ||
      !/^[a-z0-9][a-z0-9._+-]*$/.test(withoutLeadingDots)
    ) {
      return {
        error:
          "Utilisez une extension simple (lettres, chiffres, points, +, - ou _).",
      };
    }
    return { value: `.${withoutLeadingDots}` };
  }

  const value = rawValue
    .trim()
    .replace(/\\/g, "/")
    .replace(/\/{2,}/g, "/")
    .replace(/^\.\//, "")
    .replace(/\/+$/, "");
  if (!value) {
    return { error: "Indiquez un préfixe de chemin relatif à exclure." };
  }
  if (
    value.length > 240 ||
    value.startsWith("/") ||
    /^[a-z]:\//i.test(value) ||
    /[\u0000-\u001f<>:"|?*]/.test(value) ||
    value.split("/").some((segment) => segment === "." || segment === "..")
  ) {
    return {
      error:
        "Utilisez un chemin relatif sûr, sans lettre de lecteur, sans « .. », ni caractères réservés.",
    };
  }
  return { value };
}

function isMonitoringDashboard(
  value: void | MonitoringDashboard,
): value is MonitoringDashboard {
  return (
    typeof value === "object" &&
    value !== null &&
    "workspaceId" in value &&
    "counts" in value
  );
}
