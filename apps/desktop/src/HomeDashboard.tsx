import type {
  MonitoringDashboard,
  OrganizationOperation,
  RegisteredRoot,
  ScanResult,
  SystemStatus,
} from "./types";
import { recordBetaMetric } from "./betaMetrics";
import "./SimpleBetaUx.css";

export type AppDestination =
  | "home"
  | "scan"
  | "search"
  | "review"
  | "organization"
  | "monitoring"
  | "rules"
  | "history";

export type PrimaryAction = {
  label: string;
  destination?: AppDestination;
  run?: "selectFolder" | "startScan" | "analyze" | "ranger";
};

export const MAX_RECENT_ACTIVITY = 8;
export const MAX_RECENT_PROPOSALS = 6;

export type OrganizationHealth = {
  kind: "unavailable" | "empty" | "categorical";
  label: string;
  detail: string;
  percentage: number | null;
  tone: "neutral" | "good" | "watch" | "attention";
};

export function resolveOrganizationHealth(input: {
  filesAnalyzed: number | null;
  needsReview: number | null;
  countsAvailable: boolean;
}): OrganizationHealth {
  const { filesAnalyzed, needsReview, countsAvailable } = input;
  if (!countsAvailable || filesAnalyzed === null || needsReview === null) {
    return {
      kind: "unavailable",
      label: "État indisponible",
      detail: "Les agrégats d’organisation ne sont pas encore disponibles.",
      percentage: null,
      tone: "neutral",
    };
  }
  if (filesAnalyzed <= 0) {
    return {
      kind: "empty",
      label: "Pas encore d’analyse",
      detail: "Analysez un dossier pour évaluer l’organisation proposée.",
      percentage: null,
      tone: "neutral",
    };
  }
  const resolved = Math.max(0, filesAnalyzed - needsReview);
  const percentage = Math.round((100 * resolved) / filesAnalyzed);
  if (needsReview === 0) {
    return {
      kind: "categorical",
      label: "Très bien organisé",
      detail: `${percentage} % des fichiers analysés n’ont pas d’élément de revue en suspens.`,
      percentage,
      tone: "good",
    };
  }
  if (needsReview / filesAnalyzed < 0.1) {
    return {
      kind: "categorical",
      label: "Quelques éléments à vérifier",
      detail: `${needsReview.toLocaleString()} fichier${
        needsReview === 1 ? "" : "s"
      } nécessitent votre avis (${percentage} % sans revue en suspens).`,
      percentage,
      tone: "watch",
    };
  }
  return {
    kind: "categorical",
    label: "Attention requise",
    detail: `${needsReview.toLocaleString()} élément${
      needsReview === 1 ? "" : "s"
    } à vérifier sur ${filesAnalyzed.toLocaleString()} analysés.`,
    percentage,
    tone: "attention",
  };
}

export function monitoringLabel(dashboard: MonitoringDashboard | null): string {
  if (!dashboard) {
    return "Non disponible";
  }
  if (dashboard.paused) {
    return "En pause";
  }
  const unhealthy = dashboard.folders.find((folder) =>
    /degraded|overflow|offline|error|paused/i.test(folder.status),
  );
  if (unhealthy) {
    return unhealthy.status;
  }
  if (dashboard.folders.some((folder) => folder.enabled)) {
    return "Active";
  }
  if (dashboard.folders.length === 0) {
    return "Aucun dossier surveillé";
  }
  return "En veille";
}

export function monitoringHasIssue(
  dashboard: MonitoringDashboard | null,
): boolean {
  if (!dashboard) {
    return false;
  }
  if (dashboard.paused) {
    return true;
  }
  return dashboard.folders.some((folder) =>
    /degraded|overflow|offline|error|paused/i.test(folder.status),
  );
}

export function embeddingStatusLabel(): string {
  return "—";
}

export function formatProposedDestination(operation: OrganizationOperation): string {
  const parts = operation.proposedDestination.filter(Boolean);
  if (parts.length === 0) {
    return operation.proposedRelativePath || "À vérifier";
  }
  return parts.join(" / ");
}

export function resolvePrimaryAction(input: {
  root: RegisteredRoot | null;
  scan: ScanResult | null;
  dashboard: MonitoringDashboard | null;
  contentNeedsReview: number | null;
  organized?: boolean;
}): PrimaryAction {
  if (input.organized) {
    return { label: "Relancer le rangement", run: "ranger" };
  }
  return { label: "Ranger mon ordinateur", run: "ranger" };
}

type HomeDashboardProps = {
  loading: boolean;
  system: SystemStatus | null;
  workspaceId: string | null;
  root: RegisteredRoot | null;
  scan: ScanResult | null;
  dashboard: MonitoringDashboard | null;
  dashboardError: boolean;
  contentNeedsReview: number | null;
  contentFailed: number | null;
  contentUnsupported: number | null;
  organized?: boolean;
  organizedCount?: number | null;
  onPrimaryAction: (action: PrimaryAction) => void;
  onNavigate: (destination: AppDestination) => void;
  onSearch: (query: string) => void;
  onRetryDashboard: () => void;
  onChooseFolders?: () => void;
};

export function HomeDashboard({
  loading,
  workspaceId,
  organized = false,
  organizedCount = null,
  onPrimaryAction,
  onNavigate,
}: HomeDashboardProps) {
  const action = resolvePrimaryAction({
    root: null,
    scan: null,
    dashboard: null,
    contentNeedsReview: null,
    organized,
  });

  function startOrganization() {
    recordBetaMetric("organization_started");
    onPrimaryAction(action);
  }

  function openSearch() {
    recordBetaMetric("search_opened");
    onNavigate("search");
  }

  if (loading) {
    return (
      <section className="home-dashboard home-dashboard--simple" aria-labelledby="home-title">
        <p className="home-loading">Chargement…</p>
      </section>
    );
  }

  if (organized) {
    return (
      <section className="home-dashboard home-dashboard--simple home-dashboard--minimal" aria-labelledby="home-title">
        <h2 id="home-title">Votre ordinateur est rangé.</h2>
        <p className="home-promise">
          {organizedCount != null
            ? `${organizedCount.toLocaleString()} fichiers ont été organisés.`
            : "Vos fichiers ont été organisés."}
        </p>
        <div className="home-main-actions">
          <button
            type="button"
            className="primary home-primary-cta"
            onClick={startOrganization}
          >
            Relancer le rangement
          </button>
          {workspaceId ? (
            <button
              type="button"
              className="home-search-cta"
              onClick={openSearch}
            >
              Rechercher un fichier
            </button>
          ) : null}
        </div>
      </section>
    );
  }

  return (
    <section className="home-dashboard home-dashboard--simple home-dashboard--minimal" aria-labelledby="home-title">
      <h2 id="home-title">Votre ordinateur est en bazar ?</h2>
      <p className="home-promise">
        ZEMO range vos fichiers personnels sans toucher à vos applications.
      </p>
      <p className="home-preview-promise">
        Avant tout changement, ZEMO vous montre seulement les dossiers qu’il veut créer.
      </p>
      <div className="home-main-actions">
        <button
          type="button"
          className="primary home-primary-cta"
          onClick={startOrganization}
        >
          Ranger mon ordinateur
        </button>
        {workspaceId ? (
          <button
            type="button"
            className="home-search-cta"
            onClick={openSearch}
          >
            Rechercher un fichier
          </button>
        ) : null}
      </div>
    </section>
  );
}
