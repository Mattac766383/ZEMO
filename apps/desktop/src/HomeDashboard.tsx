import { useEffect, useId, useState, type FormEvent } from "react";
import {
  activateLocalEmbeddingModel,
  getEmbeddingModelStatus,
  getErrorMessage,
  getLatestOrganizationProposal,
  getRulesPreferences,
} from "./api";
import type {
  EmbeddingModelStatus,
  MonitoringActivity,
  MonitoringDashboard,
  OrganizationOperation,
  OrganizationProposalSummary,
  RegisteredRoot,
  ScanResult,
  SystemStatus,
} from "./types";

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
  run?: "selectFolder" | "startScan" | "analyze";
};

export const MAX_RECENT_ACTIVITY = 8;
export const MAX_RECENT_PROPOSALS = 6;

export type OrganizationHealth = {
  kind: "unavailable" | "empty" | "categorical";
  label: string;
  detail: string;
  /** Percentage only when a deterministic formula can be applied. */
  percentage: number | null;
  tone: "neutral" | "good" | "watch" | "attention";
};

/**
 * Organization health formula (deterministic, aggregate-only):
 *   eligible = filesAnalyzed (monitoring dashboard counts)
 *   unresolved = needsReview
 *   percentage = round(100 * (eligible - unresolved) / eligible)
 * when eligible > 0 and counts are available.
 * Categorical labels follow the same ratio; no AI score is invented.
 */
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

export function embeddingStatusLabel(
  status: EmbeddingModelStatus | null | undefined,
): string {
  if (status === undefined) {
    return "—";
  }
  if (status === null) {
    return "Indisponible";
  }
  switch (status.status) {
    case "ready":
      return "Active";
    case "not_installed":
      return "Non activée";
    case "downloading":
    case "installing":
    case "loading":
      return "Activation…";
    case "corrupt":
    case "failed":
    case "incompatible_version":
      return "Erreur";
    default:
      return status.status;
  }
}

export function formatProposedDestination(operation: OrganizationOperation): string {
  const parts = operation.proposedDestination.filter(Boolean);
  if (parts.length === 0) {
    return operation.proposedRelativePath || "TO_REVIEW / Non classés";
  }
  return parts.join(" / ");
}

export function resolvePrimaryAction(input: {
  root: RegisteredRoot | null;
  scan: ScanResult | null;
  dashboard: MonitoringDashboard | null;
  contentNeedsReview: number | null;
}): PrimaryAction {
  const { root, scan, dashboard, contentNeedsReview } = input;
  if (!root) {
    return { label: "Organiser mon ordinateur", run: "selectFolder" };
  }
  if (!scan || scan.status === "CANCELLED") {
    return { label: "Organiser mon ordinateur", run: "startScan" };
  }
  const needsReview =
    dashboard?.counts.needsReview ?? contentNeedsReview ?? 0;
  if (needsReview > 0) {
    return {
      label:
        needsReview === 1
          ? "Vérifier 1 élément"
          : `Vérifier ${needsReview.toLocaleString()} éléments`,
      destination: "review",
    };
  }
  if ((dashboard?.counts.readyToOrganize ?? 0) > 0) {
    return { label: "Voir l’organisation proposée", destination: "organization" };
  }
  if ((dashboard?.counts.pendingProposals ?? 0) > 0) {
    return { label: "Voir l’organisation proposée", destination: "organization" };
  }
  if (
    scan &&
    ["COMPLETED", "COMPLETED_WITH_ERRORS"].includes(scan.status) &&
    contentNeedsReview === null &&
    !dashboard
  ) {
    return { label: "Voir l’organisation proposée", destination: "organization" };
  }
  if (scan && ["COMPLETED", "COMPLETED_WITH_ERRORS"].includes(scan.status)) {
    return { label: "Voir l’organisation proposée", destination: "organization" };
  }
  return { label: "Rechercher un fichier", destination: "search" };
}

type AttentionCard = {
  id: string;
  title: string;
  count: number | null;
  explanation: string;
  destination: AppDestination;
  actionLabel: string;
  tone: "attention" | "watch" | "neutral";
};

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
  onPrimaryAction: (action: PrimaryAction) => void;
  onNavigate: (destination: AppDestination) => void;
  onSearch: (query: string) => void;
  onRetryDashboard: () => void;
};

export function HomeDashboard({
  loading,
  system,
  workspaceId,
  root,
  scan,
  dashboard,
  dashboardError,
  contentNeedsReview,
  contentFailed,
  contentUnsupported,
  onPrimaryAction,
  onNavigate,
  onSearch,
  onRetryDashboard,
}: HomeDashboardProps) {
  const searchId = useId();
  const [searchDraft, setSearchDraft] = useState("");
  const [embeddingStatus, setEmbeddingStatus] = useState<
    EmbeddingModelStatus | null | undefined
  >(undefined);
  const [ruleSuggestionCount, setRuleSuggestionCount] = useState<number | null>(
    null,
  );
  const [proposalSummary, setProposalSummary] =
    useState<OrganizationProposalSummary | null>(null);
  const [recentOperations, setRecentOperations] = useState<
    OrganizationOperation[] | null
  >(null);
  const [supplementalLoading, setSupplementalLoading] = useState(false);
  const [embeddingBusy, setEmbeddingBusy] = useState(false);
  const [embeddingMessage, setEmbeddingMessage] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void getEmbeddingModelStatus()
      .then((status) => {
        if (active) {
          setEmbeddingStatus(status);
        }
      })
      .catch(() => {
        if (active) {
          setEmbeddingStatus(null);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    if (!workspaceId || !root) {
      setRuleSuggestionCount(null);
      setProposalSummary(null);
      setRecentOperations(null);
      setSupplementalLoading(false);
      return;
    }
    setSupplementalLoading(true);
    void Promise.all([
      getRulesPreferences(workspaceId)
        .then((state) =>
          state.suggestions.filter((item) => item.status === "pending").length,
        )
        .catch(() => null),
      getLatestOrganizationProposal(workspaceId, root.id, {
        uiBound: true,
        operationLimit: MAX_RECENT_PROPOSALS,
      })
        .then((proposal) => ({
          summary: proposal.summary,
          operations: proposal.operations.slice(0, MAX_RECENT_PROPOSALS),
        }))
        .catch(() => null),
    ])
      .then(([suggestions, proposal]) => {
        if (!active) {
          return;
        }
        setRuleSuggestionCount(suggestions);
        setProposalSummary(proposal?.summary ?? null);
        setRecentOperations(proposal?.operations ?? null);
      })
      .finally(() => {
        if (active) {
          setSupplementalLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [workspaceId, root]);

  const action = resolvePrimaryAction({
    root,
    scan,
    dashboard,
    contentNeedsReview,
  });
  const counts = dashboard?.counts;
  const filesAnalyzed =
    counts?.filesAnalyzed ??
    (scan && ["COMPLETED", "COMPLETED_WITH_ERRORS"].includes(scan.status)
      ? scan.filesIndexed
      : null);
  const needsReview =
    counts?.needsReview ??
    (contentNeedsReview !== null ? contentNeedsReview : null);
  const attentionTotal =
    needsReview !== null
      ? needsReview
      : contentNeedsReview !== null
        ? contentNeedsReview
        : null;
  const health = resolveOrganizationHealth({
    filesAnalyzed: counts?.filesAnalyzed ?? filesAnalyzed,
    needsReview: counts ? counts.needsReview : needsReview,
    countsAvailable: Boolean(counts),
  });

  const unhealthyFolders =
    dashboard?.folders.filter((folder) =>
      /degraded|overflow|offline|error|paused/i.test(folder.status),
    ) ?? [];
  const extractionProblems =
    contentFailed !== null || contentUnsupported !== null
      ? (contentFailed ?? 0) + (contentUnsupported ?? 0)
      : null;
  const unclassifiedCount =
    proposalSummary?.needsReview ??
    (counts?.needsReview !== undefined ? counts.needsReview : null);

  const attentionCards: AttentionCard[] = [];
  if ((needsReview ?? 0) > 0) {
    attentionCards.push({
      id: "review",
      title: "Documents à vérifier",
      count: needsReview,
      explanation:
        "Ambiguïtés ou faible confiance : à classer tant que vous n’avez pas tranché.",
      destination: "review",
      actionLabel: "Ouvrir À revoir",
      tone: "attention",
    });
  }
  if ((extractionProblems ?? 0) > 0) {
    attentionCards.push({
      id: "extraction",
      title: "Problèmes d’extraction",
      count: extractionProblems,
      explanation:
        "Fichiers non lus ou partiellement lus : la proposition d’organisation peut être incomplète.",
      destination: "scan",
      actionLabel: "Voir l’analyse",
      tone: "watch",
    });
  }
  if (
    proposalSummary &&
    proposalSummary.needsReview > 0 &&
    (needsReview === null || proposalSummary.needsReview !== needsReview)
  ) {
    attentionCards.push({
      id: "unclassified",
      title: "Fichiers non classés",
      count: unclassifiedCount,
      explanation:
        "Destinations proposées sous À vérifier — rien n’a été déplacé.",
      destination: "organization",
      actionLabel: "Voir l’organisation",
      tone: "watch",
    });
  }
  if ((ruleSuggestionCount ?? 0) > 0) {
    attentionCards.push({
      id: "rules",
      title: "Préférences de rangement",
      count: ruleSuggestionCount,
      explanation:
        "Suggestions locales à accepter ou ignorer. Elles n’autorisent aucun déplacement automatique.",
      destination: "rules",
      actionLabel: "Ouvrir les préférences",
      tone: "neutral",
    });
  }
  if (dashboard?.paused) {
    attentionCards.push({
      id: "monitoring-paused",
      title: "Surveillance en pause",
      count: null,
      explanation:
        "Aucun nouveau fichier ne sera proposé tant que la surveillance est en pause.",
      destination: "monitoring",
      actionLabel: "Ouvrir la surveillance",
      tone: "watch",
    });
  } else if (unhealthyFolders.length > 0) {
    attentionCards.push({
      id: "monitoring-issue",
      title: "Dossier de surveillance indisponible",
      count: unhealthyFolders.length,
      explanation:
        "Un dossier suivi est hors ligne ou dégradé. Les propositions peuvent être en retard.",
      destination: "monitoring",
      actionLabel: "Ouvrir la surveillance",
      tone: "attention",
    });
  }

  const recentActivity = (dashboard?.recentActivity ?? []).slice(
    0,
    MAX_RECENT_ACTIVITY,
  );
  const monitoredCount =
    dashboard?.folders.filter((folder) => folder.enabled).length ?? null;
  const modelLabel = embeddingStatusLabel(embeddingStatus);
  const canActivateModel =
    embeddingStatus?.status === "not_installed" ||
    embeddingStatus?.status === "failed" ||
    embeddingStatus?.status === "corrupt" ||
    embeddingStatus?.status === "incompatible_version";

  function handleSearchSubmit(event: FormEvent) {
    event.preventDefault();
    onSearch(searchDraft.trim());
  }

  if (!loading && !root) {
    return (
      <section className="home-dashboard home-dashboard--empty" aria-labelledby="home-title">
        <header className="home-dashboard__header">
          <div>
            <span className="eyebrow">Accueil</span>
            <h2 id="home-title">Bonjour</h2>
            <p className="home-promise">Organisez et retrouvez vos fichiers.</p>
            <p>
              Choisissez ce que vous voulez analyser. L’application analyse
              localement, propose une organisation, puis vous aide à retrouver
              vos fichiers. Aucun fichier n’est déplacé automatiquement.
            </p>
          </div>
          <button
            type="button"
            className="primary home-primary-cta"
            onClick={() => onPrimaryAction(action)}
          >
            Organiser mon ordinateur
          </button>
        </header>
        <p className="home-privacy-note">
          L’analyse de vos fichiers et vos recherches s’effectuent localement.
          Une connexion peut être utilisée pour télécharger les composants du
          modèle lorsque vous le demandez.
        </p>
      </section>
    );
  }

  return (
    <section className="home-dashboard" aria-labelledby="home-title">
      <header className="home-dashboard__header">
        <div>
          <span className="eyebrow">Accueil</span>
          <h2 id="home-title">Bonjour</h2>
          <p className="home-promise">
            {filesAnalyzed !== null && filesAnalyzed > 0
              ? "Votre ordinateur a été analysé"
              : "Prêt à analyser votre ordinateur"}
          </p>
          <p className="home-hero-status" aria-live="polite">
            <span className="home-hero-stat">
              <strong>
                {filesAnalyzed === null ? "—" : filesAnalyzed.toLocaleString()}
              </strong>{" "}
              fichiers analysés
            </span>
            <span className="home-hero-stat home-hero-stat--attention">
              <strong>
                {attentionTotal === null
                  ? "—"
                  : attentionTotal.toLocaleString()}
              </strong>{" "}
              à vérifier
            </span>
          </p>
          <p className="home-hero-monitoring">
            Surveillance :{" "}
            <span
              className={
                monitoringHasIssue(dashboard)
                  ? "home-status-dot home-status-dot--issue"
                  : "home-status-dot home-status-dot--ok"
              }
            >
              {monitoringLabel(dashboard) === "Active"
                ? "Surveillance active"
                : monitoringLabel(dashboard) === "En pause"
                  ? "Surveillance en pause"
                  : monitoringLabel(dashboard)}
            </span>
          </p>
        </div>
        <button
          type="button"
          className="primary home-primary-cta"
          onClick={() => onPrimaryAction(action)}
        >
          {action.label}
        </button>
      </header>

      {loading ? (
        <div className="home-loading-block" role="status" aria-busy="true">
          <div className="home-skeleton home-skeleton--wide" />
          <div className="home-skeleton" />
          <div className="home-skeleton home-skeleton--short" />
          <p className="home-loading">Chargement…</p>
        </div>
      ) : null}

      {dashboardError ? (
        <div className="home-error" role="alert">
          <p>Impossible de charger l’état d’accueil.</p>
          <button type="button" onClick={onRetryDashboard}>
            Réessayer
          </button>
        </div>
      ) : null}

      <div className="home-row home-row--status">
        <section
          className={`home-card home-health home-health--${health.tone}`}
          aria-labelledby="home-health-title"
        >
          <h3 id="home-health-title">État de l’organisation</h3>
          <p className="home-health__label">{health.label}</p>
          {health.percentage !== null ? (
            <p className="home-health__pct" title="(fichiers analysés − à revoir) / fichiers analysés">
              {health.percentage} %
            </p>
          ) : (
            <p className="home-health__pct home-health__pct--na">—</p>
          )}
          <p className="home-health__detail">{health.detail}</p>
          <dl className="home-health__stats">
            <div>
              <dt>Prêts (forte confiance)</dt>
              <dd>
                {counts
                  ? counts.readyToOrganize.toLocaleString()
                  : proposalSummary
                    ? proposalSummary.highConfidence.toLocaleString()
                    : "—"}
              </dd>
            </div>
            <div>
              <dt>À vérifier</dt>
              <dd>{needsReview === null ? "—" : needsReview.toLocaleString()}</dd>
            </div>
            <div>
              <dt>Non pris en charge / erreur</dt>
              <dd>
                {extractionProblems === null
                  ? "—"
                  : extractionProblems.toLocaleString()}
              </dd>
            </div>
          </dl>
        </section>

        <section
          className="home-card home-attention"
          aria-labelledby="home-attention-title"
        >
          <h3 id="home-attention-title">À vérifier</h3>
          {attentionCards.length === 0 ? (
            <p className="empty-state">
              {root
                ? "Rien d’urgent pour le moment. Vous pouvez rechercher un fichier ou examiner l’organisation proposée."
                : "Choisissez un dossier pour commencer."}
            </p>
          ) : (
            <ul className="home-attention-list">
              {attentionCards.map((card) => (
                <li
                  key={card.id}
                  className={`home-attention-card home-attention-card--${card.tone}`}
                >
                  <div>
                    <strong>
                      {card.count !== null
                        ? `${card.count.toLocaleString()} · ${card.title}`
                        : card.title}
                    </strong>
                    <p>{card.explanation}</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => onNavigate(card.destination)}
                  >
                    {card.actionLabel}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      <section className="home-card home-search" aria-labelledby="home-search-title">
        <h3 id="home-search-title">Recherche rapide</h3>
        <p>Retrouvez un fichier par nom ou par sens, sans quitter l’accueil.</p>
        <form className="home-search-form" onSubmit={handleSearchSubmit}>
          <label className="sr-only" htmlFor={searchId}>
            Rechercher une facture, une photo, un devis
          </label>
          <input
            id={searchId}
            type="search"
            value={searchDraft}
            onChange={(event) => setSearchDraft(event.target.value)}
            placeholder="Rechercher une facture, une photo, un devis…"
            aria-describedby={`${searchId}-hint`}
          />
          <button type="submit" className="primary">
            Rechercher
          </button>
        </form>
        <p id={`${searchId}-hint`} className="home-search-hint">
          Même sans connaître le nom exact du fichier.
        </p>
      </section>

      <div className="home-row home-row--activity">
        <section
          className="home-card"
          aria-labelledby="home-activity-title"
        >
          <h3 id="home-activity-title">Activité récente</h3>
          {!dashboard && !dashboardError ? (
            <p className="empty-state">
              {root
                ? "Aucune activité de surveillance chargée pour le moment."
                : "Choisissez un dossier pour commencer."}
            </p>
          ) : recentActivity.length === 0 ? (
            <p className="empty-state">
              Pas encore d’activité récente. Lancez un scan ou activez la
              surveillance.
            </p>
          ) : (
            <ul className="home-activity-list">
              {recentActivity.map((item) => (
                <li key={item.id}>
                  <strong>{activityHeadline(item)}</strong>
                  <span>{activityDetail(item)}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section
          className="home-card"
          aria-labelledby="home-proposals-title"
        >
          <h3 id="home-proposals-title">Propositions récentes</h3>
          {supplementalLoading && recentOperations === null ? (
            <p className="home-loading" role="status">
              Chargement des propositions…
            </p>
          ) : !recentOperations || recentOperations.length === 0 ? (
            <p className="empty-state">
              Aucune proposition d’organisation disponible. Générez-en une
              depuis Organisation.
            </p>
          ) : (
            <ul className="home-proposal-list">
              {recentOperations.map((operation) => (
                <li key={operation.id}>
                  <strong>{operation.sourceName}</strong>
                  <span className="home-proposal-current">
                    Emplacement actuel : {operation.sourceRelativePath}
                  </span>
                  <span className="home-proposal-proposed">
                    Destination proposée : {formatProposedDestination(operation)}
                  </span>
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            className="home-inline-link"
            onClick={() => onNavigate("organization")}
          >
            Voir l’organisation proposée
          </button>
        </section>
      </div>

      <div className="home-row home-row--system">
        <section
          className="home-card home-monitoring-card"
          aria-labelledby="home-monitoring-title"
        >
          <h3 id="home-monitoring-title">Surveillance</h3>
          <p className="home-monitoring-status">
            <span
              className={
                monitoringHasIssue(dashboard)
                  ? "home-status-dot home-status-dot--issue"
                  : "home-status-dot home-status-dot--ok"
              }
              aria-hidden="true"
            />
            <strong>{monitoringLabel(dashboard)}</strong>
          </p>
          <p>
            {dashboard
              ? `${(monitoredCount ?? 0).toLocaleString()} dossier${
                  (monitoredCount ?? 0) === 1 ? "" : "s"
                } surveillé${(monitoredCount ?? 0) === 1 ? "" : "s"}`
              : "État de surveillance indisponible"}
          </p>
          <p className="home-monitoring-honesty">
            La surveillance prépare des propositions uniquement — elle ne
            déplace pas les fichiers automatiquement.
          </p>
          {(needsReview ?? 0) > 0 ? (
            <p>
              {(needsReview ?? 0).toLocaleString()} fichier
              {(needsReview ?? 0) === 1 ? "" : "s"} en attente de revue
            </p>
          ) : null}
          <button type="button" onClick={() => onNavigate("monitoring")}>
            Ouvrir la surveillance
          </button>
        </section>

        <section
          className="home-card"
          aria-labelledby="home-ai-title"
        >
          <h3 id="home-ai-title">Améliorer la recherche</h3>
          <p className="home-ai-status">
            <strong>{modelLabel}</strong>
          </p>
          <p>
            Retrouvez des fichiers même sans connaître leur nom exact (~118 Mo,
            traitement local).
          </p>
          {embeddingMessage ? (
            <p className="home-loading" role="status">
              {embeddingMessage}
            </p>
          ) : null}
          {canActivateModel ? (
            <button
              type="button"
              disabled={embeddingBusy}
              onClick={() => {
                setEmbeddingBusy(true);
                setEmbeddingMessage("Activation…");
                void activateLocalEmbeddingModel()
                  .then((status) => {
                    setEmbeddingStatus(status);
                    setEmbeddingMessage(
                      status.status === "ready"
                        ? "Recherche améliorée active."
                        : null,
                    );
                  })
                  .catch((reason) => {
                    setEmbeddingMessage(getErrorMessage(reason));
                  })
                  .finally(() => {
                    setEmbeddingBusy(false);
                  });
              }}
            >
              {embeddingBusy ? "Activation…" : "Activer"}
            </button>
          ) : (
            <button type="button" onClick={() => onNavigate("search")}>
              Ouvrir la recherche
            </button>
          )}
        </section>

        <section
          className="home-card home-privacy-card"
          aria-labelledby="home-privacy-title"
        >
          <h3 id="home-privacy-title">Confidentialité</h3>
          <p>
            <strong>
              {system?.localFirst ? "Analyse locale" : "État local…"}
            </strong>
          </p>
          <p>
            L’analyse de vos fichiers et vos recherches s’effectuent
            localement. Une connexion peut être utilisée pour télécharger les
            composants du modèle lorsque vous le demandez.
          </p>
          <p className="home-privacy-note">
            Aucune modification automatique du système de fichiers n’est
            effectuée depuis cet écran.
          </p>
        </section>
      </div>

      <section
        className="home-card home-loop"
        aria-labelledby="home-loop-title"
      >
        <h3 id="home-loop-title">Ce que fait l’assistant</h3>
        <ol className="home-loop-steps">
          <li>Nouveau fichier</li>
          <li>Analyse locale</li>
          <li>Proposition</li>
          <li>Vérification si nécessaire</li>
          <li>Recherche immédiate</li>
        </ol>
      </section>

      {root ? (
        <p className="home-folder-footer">
          Dossier suivi : <code>{root.selectedPath}</code>
          {scan ? (
            <>
              {" "}
              · Dernier scan : {scan.filesIndexed.toLocaleString()} fichiers (
              {scan.status})
            </>
          ) : null}
        </p>
      ) : null}
    </section>
  );
}

function activityHeadline(item: MonitoringActivity): string {
  if (item.failed > 0) {
    return `${item.failed.toLocaleString()} problème${
      item.failed === 1 ? "" : "s"
    } de surveillance`;
  }
  if (item.filesAnalyzed > 0) {
    return `${item.filesAnalyzed.toLocaleString()} nouveau${
      item.filesAnalyzed === 1 ? "" : "x"
    } fichier${item.filesAnalyzed === 1 ? "" : "s"} détecté${
      item.filesAnalyzed === 1 ? "" : "s"
    }`;
  }
  return item.summary;
}

function activityDetail(item: MonitoringActivity): string {
  const parts: string[] = [];
  if (item.readyToOrganize > 0) {
    parts.push(
      `${item.readyToOrganize.toLocaleString()} compris avec forte confiance`,
    );
  }
  if (item.needsReview > 0) {
    parts.push(
      `${item.needsReview.toLocaleString()} nécessitent votre avis`,
    );
  }
  if (item.failed > 0) {
    parts.push(`${item.failed.toLocaleString()} en échec`);
  }
  if (parts.length === 0) {
    return item.summary;
  }
  return parts.join(" · ");
}
