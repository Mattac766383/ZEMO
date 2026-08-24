import { useEffect, useMemo, useState } from "react";
import {
  cancelExtractionRetry,
  getErrorMessage,
  listReviewItems,
  retryExtraction,
  updateReviewItem,
} from "./api";
import type {
  FileReviewItem,
  FileReviewPage,
  ReviewReasonFilter,
  ReviewStatusFilter,
} from "./types";
import "./ReviewViewV2.css";

const PAGE_SIZE = 500;
const GROUP_ACTION_BATCH = 8;
const REASON_FILTERS: Array<{ value: ReviewReasonFilter; label: string }> = [
  { value: "all", label: "Tous" },
  { value: "ocr", label: "Lecture des scans" },
  { value: "unsupported", label: "Formats" },
  { value: "permissions", label: "Accès" },
  { value: "partial", label: "Lecture partielle" },
  { value: "corrupt", label: "Fichiers endommagés" },
  { value: "semantic", label: "Compréhension" },
];

interface ReviewViewProps {
  workspaceId: string;
  onOpenFile: (fileId: string) => void;
}

interface DecisionGroup {
  key: string;
  reason: string;
  explanation: string;
  items: FileReviewItem[];
}

export function ReviewView({ workspaceId, onOpenFile }: ReviewViewProps) {
  const [status, setStatus] = useState<ReviewStatusFilter>("needs_review");
  const [reason, setReason] = useState<ReviewReasonFilter>("all");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<FileReviewPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [workingId, setWorkingId] = useState<string | null>(null);
  const [retryingId, setRetryingId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    void listReviewItems(workspaceId, status, reason, PAGE_SIZE, offset)
      .then((next) => {
        if (active) {
          setPage(next);
        }
      })
      .catch((cause) => {
        if (active) {
          setError(getErrorMessage(cause));
        }
      })
      .finally(() => {
        if (active) {
          setLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [offset, reason, refresh, status, workspaceId]);

  const decisions = useMemo(() => groupReviewItems(page?.items ?? []), [page?.items]);
  const isActiveDecisions = status === "needs_review";

  async function changeState(reviewId: string, action: "resolve" | "ignore") {
    setWorkingId(reviewId);
    setError(null);
    setNotice(null);
    try {
      await updateReviewItem(reviewId, action);
      setNotice(action === "resolve" ? "Décision enregistrée." : "Élément ignoré.");
      setRefresh((value) => value + 1);
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setWorkingId(null);
    }
  }

  async function changeGroupState(group: DecisionGroup, action: "resolve" | "ignore") {
    setWorkingId(group.key);
    setError(null);
    setNotice(
      `ZEMO applique cette décision à ${group.items.length.toLocaleString()} fichier${group.items.length === 1 ? "" : "s"}…`,
    );
    let failures = 0;
    try {
      for (let index = 0; index < group.items.length; index += GROUP_ACTION_BATCH) {
        const batch = group.items.slice(index, index + GROUP_ACTION_BATCH);
        const results = await Promise.allSettled(
          batch.map((item) => updateReviewItem(item.reviewId, action)),
        );
        failures += results.filter((result) => result.status === "rejected").length;
      }
      if (failures === 0) {
        setNotice(
          action === "resolve"
            ? `${group.items.length.toLocaleString()} fichier${group.items.length === 1 ? "" : "s"} traité${group.items.length === 1 ? "" : "s"} avec une seule décision.`
            : `${group.items.length.toLocaleString()} signal${group.items.length === 1 ? "" : "s"} ignoré${group.items.length === 1 ? "" : "s"}.`,
        );
      } else {
        setError(`${failures} élément${failures === 1 ? "" : "s"} n’ont pas pu être mis à jour.`);
      }
      setRefresh((value) => value + 1);
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setWorkingId(null);
    }
  }

  async function retry(reviewId: string) {
    setRetryingId(reviewId);
    setError(null);
    setNotice("Nouvelle lecture locale en cours…");
    try {
      const outcome = await retryExtraction(reviewId);
      setNotice(outcome.message);
      setRefresh((value) => value + 1);
    } catch (cause) {
      setError(getErrorMessage(cause));
      setNotice(null);
    } finally {
      setRetryingId(null);
    }
  }

  async function cancelRetry(reviewId: string) {
    try {
      await cancelExtractionRetry(reviewId);
      setNotice("Annulation demandée…");
    } catch (cause) {
      setError(getErrorMessage(cause));
    }
  }

  return (
    <main className="decisions-v2" aria-labelledby="decisions-v2-title">
      <header className="decisions-v2__header">
        <div>
          <span className="step">Décisions</span>
          <h2 id="decisions-v2-title">
            {isActiveDecisions ? "ZEMO a presque terminé" : "Historique des décisions"}
          </h2>
          <p>
            {isActiveDecisions
              ? "ZEMO regroupe les fichiers qui ont le même problème. Une réponse peut régler des dizaines ou des centaines de fichiers d’un coup."
              : "Consultez les signaux déjà traités sans encombrer votre rangement quotidien."}
          </p>
        </div>
        <div className="decisions-v2__count" aria-live="polite">
          <strong>{loading ? "—" : decisions.length.toLocaleString()}</strong>
          <span>décision{decisions.length === 1 ? "" : "s"}</span>
          {!loading && page?.total ? (
            <small>{page.total.toLocaleString()} fichier{page.total === 1 ? "" : "s"} concerné{page.total === 1 ? "" : "s"}</small>
          ) : null}
        </div>
      </header>

      {isActiveDecisions && !loading && decisions.length > 0 ? (
        <div className="decisions-v2__promise" role="status">
          <strong>Pas de liste interminable.</strong>
          <span>
            ZEMO vous montre seulement les décisions qu’il ne peut pas résoudre de façon sûre tout seul.
          </span>
        </div>
      ) : null}

      <details className="decisions-v2__filters">
        <summary>Filtres et historique</summary>
        <div className="decisions-v2__toolbar">
          <div className="review-filters" aria-label="Catégories de vérification">
            {REASON_FILTERS.map((filter) => (
              <button
                type="button"
                key={filter.value}
                className={reason === filter.value ? "filter-chip filter-chip--active" : "filter-chip"}
                aria-pressed={reason === filter.value}
                onClick={() => {
                  setOffset(0);
                  setReason(filter.value);
                }}
              >
                {filter.label}
              </button>
            ))}
          </div>
          <label className="status-filter">
            <span>État</span>
            <select
              value={status}
              onChange={(event) => {
                setOffset(0);
                setStatus(event.target.value as ReviewStatusFilter);
              }}
            >
              <option value="needs_review">Décisions nécessaires</option>
              <option value="resolved">Résolus</option>
              <option value="ignored">Ignorés</option>
              <option value="all">Tous les états</option>
            </select>
          </label>
        </div>
      </details>

      {notice ? <p className="notice-banner" role="status">{notice}</p> : null}
      {error ? <p className="inline-error" role="alert">{error}</p> : null}
      {loading ? (
        <p className="view-note" aria-live="polite">ZEMO regroupe les décisions…</p>
      ) : null}

      {!loading && !error && decisions.length === 0 ? (
        <div className="decisions-v2__empty empty-state">
          <strong>{isActiveDecisions ? "Rien à décider" : "Aucun élément dans cette vue"}</strong>
          <p>
            {isActiveDecisions
              ? "ZEMO n’a pas besoin de vous pour le moment."
              : "Changez le filtre d’état pour consulter d’autres décisions."}
          </p>
        </div>
      ) : null}

      <div className="decisions-v2__list" aria-busy={loading}>
        {decisions.map((group) => {
          const groupWorking = workingId === group.key;
          const single = group.items.length === 1;
          const first = group.items[0];
          return (
            <article className="decisions-v2__card" key={group.key}>
              <div className="decisions-v2__card-heading">
                <div className="decisions-v2__mark" aria-hidden="true">?</div>
                <div>
                  <span className="decisions-v2__reason">{reasonLabel(group.reason)}</span>
                  <h3>{single ? first.filename : decisionTitle(group)}</h3>
                  <p>{group.explanation}</p>
                </div>
                <div className="decisions-v2__group-count">
                  <strong>{group.items.length.toLocaleString()}</strong>
                  <span>fichier{group.items.length === 1 ? "" : "s"}</span>
                </div>
              </div>

              {!single ? (
                <div className="decisions-v2__samples" aria-label="Exemples de fichiers concernés">
                  {group.items.slice(0, 3).map((item) => (
                    <span key={item.reviewId}>{item.filename}</span>
                  ))}
                  {group.items.length > 3 ? <span>+ {group.items.length - 3} autres</span> : null}
                </div>
              ) : (
                <code className="decisions-v2__single-path">{first.relativePath}</code>
              )}

              {isActiveDecisions ? (
                <div className="decisions-v2__actions">
                  {single ? (
                    <SingleItemActions
                      item={first}
                      working={workingId === first.reviewId}
                      retrying={retryingId === first.reviewId}
                      blocked={workingId !== null || retryingId !== null}
                      onOpenFile={onOpenFile}
                      onRetry={retry}
                      onCancelRetry={cancelRetry}
                      onChangeState={changeState}
                    />
                  ) : (
                    <>
                      <button
                        type="button"
                        className="primary-action"
                        disabled={groupWorking || retryingId !== null}
                        onClick={() => void changeGroupState(group, "resolve")}
                      >
                        {groupWorking ? "Application…" : `Régler les ${group.items.length.toLocaleString()} fichiers`}
                      </button>
                      <button
                        type="button"
                        disabled={groupWorking || retryingId !== null}
                        onClick={() => void changeGroupState(group, "ignore")}
                      >
                        Ignorer ce groupe
                      </button>
                    </>
                  )}
                </div>
              ) : null}

              {!single ? (
                <details className="decisions-v2__files">
                  <summary>Voir les fichiers ({group.items.length.toLocaleString()})</summary>
                  <ul>
                    {group.items.map((item) => (
                      <li key={item.reviewId}>
                        <div>
                          <strong>{item.filename}</strong>
                          <code>{item.relativePath}</code>
                        </div>
                        <div>
                          <button type="button" onClick={() => onOpenFile(item.fileId)}>
                            Voir les détails
                          </button>
                          {item.status === "NEEDS_REVIEW" && item.retryAvailable ? (
                            retryingId === item.reviewId ? (
                              <button type="button" onClick={() => void cancelRetry(item.reviewId)}>
                                Annuler la tentative
                              </button>
                            ) : (
                              <button
                                type="button"
                                disabled={workingId !== null || retryingId !== null}
                                onClick={() => void retry(item.reviewId)}
                              >
                                Réessayer
                              </button>
                            )
                          ) : null}
                        </div>
                      </li>
                    ))}
                  </ul>
                </details>
              ) : first.technicalDetails ? (
                <details className="technical-details">
                  <summary>Détails techniques</summary>
                  <p>{first.technicalDetails}</p>
                </details>
              ) : null}
            </article>
          );
        })}
      </div>

      {page && (offset > 0 || page.hasMore) ? (
        <nav className="pagination" aria-label="Pages des décisions">
          <button
            type="button"
            disabled={offset === 0 || loading}
            onClick={() => setOffset((value) => Math.max(0, value - PAGE_SIZE))}
          >
            Précédent
          </button>
          <span>Lot {Math.floor(offset / PAGE_SIZE) + 1}</span>
          <button
            type="button"
            disabled={!page.hasMore || loading}
            onClick={() => setOffset((value) => value + PAGE_SIZE)}
          >
            Suivant
          </button>
        </nav>
      ) : null}
    </main>
  );
}

function SingleItemActions({
  item,
  working,
  retrying,
  blocked,
  onOpenFile,
  onRetry,
  onCancelRetry,
  onChangeState,
}: {
  item: FileReviewItem;
  working: boolean;
  retrying: boolean;
  blocked: boolean;
  onOpenFile: (fileId: string) => void;
  onRetry: (reviewId: string) => Promise<void>;
  onCancelRetry: (reviewId: string) => Promise<void>;
  onChangeState: (reviewId: string, action: "resolve" | "ignore") => Promise<void>;
}) {
  return (
    <>
      <button type="button" onClick={() => onOpenFile(item.fileId)}>Voir les détails</button>
      {item.status === "NEEDS_REVIEW" ? (
        <>
          {item.retryAvailable ? (
            retrying ? (
              <button type="button" className="danger-outline" onClick={() => void onCancelRetry(item.reviewId)}>
                Annuler la tentative
              </button>
            ) : (
              <button type="button" disabled={blocked} onClick={() => void onRetry(item.reviewId)}>
                Réessayer
              </button>
            )
          ) : null}
          <button type="button" disabled={working || retrying} onClick={() => void onChangeState(item.reviewId, "resolve")}>
            Résoudre
          </button>
          <button type="button" disabled={working || retrying} onClick={() => void onChangeState(item.reviewId, "ignore")}>
            Ignorer
          </button>
        </>
      ) : null}
    </>
  );
}

function groupReviewItems(items: FileReviewItem[]): DecisionGroup[] {
  const groups = new Map<string, DecisionGroup>();
  for (const item of items) {
    const explanationKey = normalizeDecisionText(item.explanation);
    const key = `${item.status}|${item.sourceSubsystem}|${item.reason}|${explanationKey}`;
    const current = groups.get(key);
    if (current) {
      current.items.push(item);
    } else {
      groups.set(key, {
        key,
        reason: item.reason,
        explanation: item.explanation,
        items: [item],
      });
    }
  }
  return [...groups.values()].sort((left, right) => {
    if (right.items.length !== left.items.length) return right.items.length - left.items.length;
    return left.explanation.localeCompare(right.explanation, "fr");
  });
}

function normalizeDecisionText(value: string): string {
  return value
    .toLocaleLowerCase("fr")
    .replace(/\b\d+\b/g, "#")
    .replace(/\s+/g, " ")
    .trim();
}

function decisionTitle(group: DecisionGroup): string {
  const label = reasonLabel(group.reason);
  return `${label} · ${group.items.length.toLocaleString()} fichiers concernés`;
}

function reasonLabel(value: string): string {
  const labels: Record<string, string> = {
    OCR_FAILED: "Documents scannés difficiles à lire",
    OCR_PROVIDER_UNAVAILABLE: "Lecture des documents scannés à compléter",
    UNSUPPORTED_FORMAT: "Format à traiter autrement",
    PERMISSION_DENIED: "Autorisation nécessaire",
    UNREADABLE: "Fichiers illisibles",
    PARTIAL_EXTRACTION: "Compréhension partielle",
    ENCRYPTED: "Documents protégés",
    CORRUPT: "Fichiers endommagés",
    TOO_LARGE: "Fichiers volumineux",
    TYPE_MISMATCH: "Type de fichier inattendu",
    EXTRACTION_FAILED: "Lecture à relancer",
    UNKNOWN: "Décision nécessaire",
  };
  return labels[value.toUpperCase()] ?? value.replace(/_/g, " ").toLowerCase();
}
