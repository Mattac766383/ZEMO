import { useEffect, useState } from "react";
import {
  cancelExtractionRetry,
  getErrorMessage,
  listReviewItems,
  retryExtraction,
  updateReviewItem,
} from "./api";
import type {
  FileReviewPage,
  ReviewReasonFilter,
  ReviewStatusFilter,
} from "./types";

const PAGE_SIZE = 50;
const REASON_FILTERS: Array<{ value: ReviewReasonFilter; label: string }> = [
  { value: "all", label: "Tous" },
  { value: "ocr", label: "Reconnaissance du texte" },
  { value: "unsupported", label: "Formats non pris en charge" },
  { value: "permissions", label: "Accès" },
  { value: "partial", label: "Extraction partielle" },
  { value: "corrupt", label: "Fichiers endommagés" },
  { value: "semantic", label: "Compréhension incertaine" },
];

interface ReviewViewProps {
  workspaceId: string;
  onOpenFile: (fileId: string) => void;
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

  async function changeState(reviewId: string, action: "resolve" | "ignore") {
    setWorkingId(reviewId);
    setError(null);
    setNotice(null);
    try {
      await updateReviewItem(reviewId, action);
      setNotice(action === "resolve" ? "Élément marqué comme résolu." : "Élément ignoré.");
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
    setNotice("Nouvelle extraction locale en cours…");
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
    <div className="review-surface">
      <div className="surface-heading">
        <div>
          <span className="step">À vérifier</span>
          <h2>Les fichiers qui demandent votre attention</h2>
          <p>
            Rien n’est déplacé ni modifié. Vous choisissez simplement comment traiter
            chaque signal.
          </p>
        </div>
        <div className="review-total" aria-live="polite">
          <strong>{page?.total.toLocaleString() ?? "—"}</strong>
          <span>à vérifier</span>
        </div>
      </div>

      <div className="review-toolbar">
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
            <option value="needs_review">À vérifier</option>
            <option value="resolved">Résolus</option>
            <option value="ignored">Ignorés</option>
            <option value="all">Tous les états</option>
          </select>
        </label>
      </div>

      {notice ? (
        <p className="notice-banner" role="status">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p className="inline-error" role="alert">
          {error}
        </p>
      ) : null}
      {loading ? (
        <p className="view-note" aria-live="polite">
          Chargement de la liste locale…
        </p>
      ) : null}
      {!loading && !error && page?.items.length === 0 ? (
        <div className="empty-state">
          <strong>Aucun fichier dans cette vue</strong>
          <p>Les éléments résolus ou ignorés restent consultables via le filtre d’état.</p>
        </div>
      ) : null}

      <div className="review-list" aria-busy={loading}>
        {page?.items.map((item) => {
          const working = workingId === item.reviewId;
          const retrying = retryingId === item.reviewId;
          return (
            <article className={`review-card review-card--${item.severity.toLowerCase()}`} key={item.reviewId}>
              <div className="review-card-main">
                <div className="review-file-mark" aria-hidden="true">
                  !
                </div>
                <div>
                  <div className="review-card-heading">
                    <div>
                      <h3>{item.filename}</h3>
                      <code>{item.relativePath}</code>
                    </div>
                    <span className={`review-state review-state--${item.status.toLowerCase()}`}>
                      {statusLabel(item.status)}
                    </span>
                  </div>
                  <p className="review-explanation">{item.explanation}</p>
                  <div className="result-meta">
                    <span>{reasonLabel(item.reason)}</span>
                    <span>
                      Extraction : {statusLabel(item.extractionStatus ?? "NOT_ANALYZED")}
                    </span>
                    {item.retryCount > 0 ? (
                      <span>{item.retryCount} nouvelle(s) tentative(s)</span>
                    ) : null}
                  </div>
                  {item.technicalDetails ? (
                    <details className="technical-details">
                      <summary>Détails techniques</summary>
                      <p>{item.technicalDetails}</p>
                    </details>
                  ) : null}
                </div>
              </div>
              <div className="review-actions">
                <button type="button" onClick={() => onOpenFile(item.fileId)}>
                  Voir les détails
                </button>
                {item.status === "NEEDS_REVIEW" ? (
                  <>
                    {item.retryAvailable ? (
                      retrying ? (
                        <button
                          type="button"
                          className="danger-outline"
                          onClick={() => void cancelRetry(item.reviewId)}
                        >
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
                    <button
                      type="button"
                      disabled={working || retrying}
                      onClick={() => void changeState(item.reviewId, "resolve")}
                    >
                      Résoudre
                    </button>
                    <button
                      type="button"
                      disabled={working || retrying}
                      onClick={() => void changeState(item.reviewId, "ignore")}
                    >
                      Ignorer
                    </button>
                  </>
                ) : null}
              </div>
            </article>
          );
        })}
      </div>

      {page && (offset > 0 || page.hasMore) ? (
        <nav className="pagination" aria-label="Pages des fichiers à vérifier">
          <button
            type="button"
            disabled={offset === 0 || loading}
            onClick={() => setOffset((value) => Math.max(0, value - PAGE_SIZE))}
          >
            Précédent
          </button>
          <span>Page {Math.floor(offset / PAGE_SIZE) + 1}</span>
          <button
            type="button"
            disabled={!page.hasMore || loading}
            onClick={() => setOffset((value) => value + PAGE_SIZE)}
          >
            Suivant
          </button>
        </nav>
      ) : null}
    </div>
  );
}

function statusLabel(value: string): string {
  const labels: Record<string, string> = {
    NEEDS_REVIEW: "À vérifier",
    RESOLVED: "Résolu",
    IGNORED: "Ignoré",
    SUCCESS: "Réussie",
    PARTIAL: "Partielle",
    FAILED: "Échec",
    UNSUPPORTED: "Non pris en charge",
    NOT_ANALYZED: "Non analysée",
  };
  return labels[value.toUpperCase()] ?? value.replace(/_/g, " ").toLowerCase();
}

function reasonLabel(value: string): string {
  const labels: Record<string, string> = {
    OCR_FAILED: "Document scanné partiellement illisible",
    OCR_PROVIDER_UNAVAILABLE: "Certains documents scannés n’ont pas pu être lus complètement",
    UNSUPPORTED_FORMAT: "Format non pris en charge",
    PERMISSION_DENIED: "Autorisation refusée",
    UNREADABLE: "Fichier illisible",
    PARTIAL_EXTRACTION: "Extraction partielle",
    ENCRYPTED: "Document chiffré",
    CORRUPT: "Fichier endommagé",
    TOO_LARGE: "Limite de sécurité",
    TYPE_MISMATCH: "Type inattendu",
    EXTRACTION_FAILED: "Extraction échouée",
    UNKNOWN: "Vérification nécessaire",
  };
  return labels[value.toUpperCase()] ?? value.replace(/_/g, " ").toLowerCase();
}
