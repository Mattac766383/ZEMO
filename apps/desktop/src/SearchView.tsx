import { useEffect, useState } from "react";
import {
  activateLocalEmbeddingModel,
  cancelLocalEmbeddingModelInstall,
  getEmbeddingModelStatus,
  rebuildSemanticAnnIndex,
  removeLocalEmbeddingModel,
  retryLocalEmbeddingModel,
  searchLocalFiles,
} from "./api";
import { classifyUserError, type UserFacingError } from "./errors";
import type {
  EmbeddingModelStatus,
  LocalSearchFilters,
  LocalSearchPage,
  SearchContext,
  SearchDocumentType,
  SearchExtraction,
  SearchFileType,
  SearchModified,
  SearchOcr,
  SearchSemanticStatus,
  SearchSort,
} from "./types";

const PAGE_SIZE = 50;
const INITIAL_FILTERS: LocalSearchFilters = {
  fileType: "all",
  modified: "any",
  extraction: "any",
  ocr: "any",
  documentType: "any",
  context: "any",
  semanticStatus: "any",
};

interface SearchViewProps {
  workspaceId: string;
  initialQuery?: string;
  onOpenFile: (fileId: string) => void;
}

export function SearchView({
  workspaceId,
  initialQuery,
  onOpenFile,
}: SearchViewProps) {
  const [text, setText] = useState(initialQuery ?? "");
  const [filters, setFilters] = useState<LocalSearchFilters>(INITIAL_FILTERS);
  const [sort, setSort] = useState<SearchSort>("relevance");
  const [page, setPage] = useState(0);
  const [disabledIntents, setDisabledIntents] = useState<string[]>([]);
  const [result, setResult] = useState<LocalSearchPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<UserFacingError | null>(null);
  const [modelStatus, setModelStatus] = useState<EmbeddingModelStatus | null>(
    null,
  );
  const [modelBusy, setModelBusy] = useState(false);
  const [modelMessage, setModelMessage] = useState<string | null>(null);

  useEffect(() => {
    if (typeof initialQuery === "string") {
      setText(initialQuery);
      setPage(0);
    }
  }, [initialQuery]);

  useEffect(() => {
    let active = true;
    void getEmbeddingModelStatus()
      .then((status) => {
        if (active) {
          setModelStatus(status);
        }
      })
      .catch(() => {
        if (active) {
          setModelStatus(null);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    const timer = window.setTimeout(() => {
      setLoading(true);
      setError(null);
      void searchLocalFiles(workspaceId, {
        text,
        filters,
        sort,
        page,
        pageSize: PAGE_SIZE,
        semanticSearch: true,
        disabledIntents,
      })
        .then((next) => {
          if (active) {
            setResult(next);
          }
        })
        .catch((reason) => {
          if (active) {
            setError(classifyUserError(reason, "search"));
          }
        })
        .finally(() => {
          if (active) {
            setLoading(false);
          }
        });
    }, 180);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [disabledIntents, filters, page, sort, text, workspaceId]);

  function updateFilter<Key extends keyof LocalSearchFilters>(
    key: Key,
    value: LocalSearchFilters[Key],
  ) {
    setPage(0);
    setFilters((current) => ({ ...current, [key]: value }));
  }

  async function runModelAction(
    action: () => Promise<EmbeddingModelStatus>,
  ) {
    setModelBusy(true);
    setModelMessage(null);
    try {
      const status = await action();
      setModelStatus(status);
      if (status.status === "not_installed" && !status.downloadImplemented) {
        setModelMessage(
          "Installation différée : placez les artefacts locaux (~118 Mo) puis réessayez. Aucun téléchargement automatique.",
        );
      } else if (status.lastError) {
        setModelMessage(status.lastError);
      }
    } catch (reason) {
      setModelMessage(classifyUserError(reason, "semantic").message);
    } finally {
      setModelBusy(false);
    }
  }

  const modelStatusLabel = (() => {
    switch (modelStatus?.status) {
      case "ready":
        return "Recherche améliorée active";
      case "downloading":
      case "installing":
        return "Activation en cours…";
      case "loading":
        return "Chargement…";
      case "corrupt":
      case "failed":
      case "incompatible_version":
        return "Activation impossible";
      case "unavailable":
        return "Indisponible";
      case "not_installed":
      default:
        return "Recherche améliorée non activée";
    }
  })();

  const annStatusLabel = (() => {
    switch (result?.embeddings.annIndexStatus) {
      case "ready":
        return "Prête";
      case "building":
        return "Préparation…";
      case "degraded":
        return "Indisponible";
      case "rebuild_required":
        return "À reconstruire";
      case "failed":
      case "not_available":
      default:
        return modelStatus?.status === "ready" ? "Préparation…" : "Indisponible";
    }
  })();

  const semanticBusy =
    modelStatus?.status === "downloading" ||
    modelStatus?.status === "installing" ||
    modelStatus?.status === "loading" ||
    result?.embeddings.annIndexStatus === "building";

  return (
    <div className="search-surface">
      <div className="surface-heading">
        <div>
          <span className="step">Recherche</span>
          <h2>Retrouvez vos fichiers</h2>
          <p>Retrouvez une facture, une photo, un devis…</p>
        </div>
        <span className="local-badge">Sur cet appareil</span>
      </div>

      <section className="embedding-model-panel" aria-label="Améliorer la recherche">
        <div>
          <strong>Recherche intelligente</strong>
          <p>
            {modelStatusLabel}
            {modelStatus
              ? ` · ~${(modelStatus.approximateDiskBytes / (1024 * 1024)).toFixed(0)} Mo · fonctionne localement`
              : null}
          </p>
          <p>
            Retrouvez vos fichiers même sans connaître leur nom exact.
          </p>
          <details className="search-advanced-details">
            <summary>En savoir plus</summary>
            <p>
              État : {annStatusLabel}
              {semanticBusy ? " · préparation en cours" : null}. La recherche
              classique reste disponible pendant l’activation.
            </p>
          </details>
          {modelMessage ? <p className="inline-error">{modelMessage}</p> : null}
        </div>
        <div className="embedding-model-actions">
          {modelStatus?.status !== "ready" &&
          modelStatus?.status !== "downloading" &&
          modelStatus?.status !== "installing" ? (
            <button
              type="button"
              disabled={modelBusy}
              onClick={() => {
                void runModelAction(activateLocalEmbeddingModel);
              }}
            >
              Activer
            </button>
          ) : null}
          {modelStatus?.status === "downloading" ||
          modelStatus?.status === "installing" ? (
            <button
              type="button"
              disabled={false}
              onClick={() => {
                void runModelAction(cancelLocalEmbeddingModelInstall);
              }}
            >
              Annuler
            </button>
          ) : null}
          {modelStatus?.status === "corrupt" ||
          modelStatus?.status === "failed" ||
          modelStatus?.status === "incompatible_version" ? (
            <button
              type="button"
              disabled={modelBusy}
              onClick={() => {
                void runModelAction(retryLocalEmbeddingModel);
              }}
            >
              Réessayer
            </button>
          ) : null}
          {modelStatus?.status === "ready" ? (
            <details className="search-advanced-details">
              <summary>Options avancées</summary>
              <div className="embedding-model-actions">
                <button
                  type="button"
                  disabled={modelBusy}
                  onClick={() => {
                    setModelBusy(true);
                    setModelMessage(null);
                    void rebuildSemanticAnnIndex(workspaceId)
                      .then((status) => {
                        setModelMessage(
                          status === "ready"
                            ? "Recherche améliorée reconstruite."
                            : `État : ${status}`,
                        );
                      })
                      .catch((reason) => {
                        setModelMessage(
                          classifyUserError(reason, "semantic").message,
                        );
                      })
                      .finally(() => {
                        setModelBusy(false);
                      });
                  }}
                >
                  Reconstruire
                </button>
                <button
                  type="button"
                  disabled={modelBusy}
                  onClick={() => {
                    void runModelAction(removeLocalEmbeddingModel);
                  }}
                >
                  Désactiver
                </button>
              </div>
            </details>
          ) : null}
          {modelStatus &&
          modelStatus.status !== "not_installed" &&
          modelStatus.status !== "downloading" &&
          modelStatus.status !== "installing" &&
          modelStatus.status !== "ready" ? (
            <button
              type="button"
              disabled={modelBusy}
              onClick={() => {
                void runModelAction(removeLocalEmbeddingModel);
              }}
            >
              Désactiver
            </button>
          ) : null}
        </div>
      </section>

      <form
        className="search-box"
        role="search"
        onSubmit={(event) => event.preventDefault()}
      >
        <label htmlFor="local-search-input">Recherche</label>
        <div>
          <span aria-hidden="true">⌕</span>
          <input
            id="local-search-input"
            type="search"
            maxLength={512}
            autoComplete="off"
            placeholder="Rechercher une facture, une photo, un devis…"
            value={text}
            onChange={(event) => {
              setPage(0);
              setDisabledIntents([]);
              setText(event.target.value);
            }}
          />
        </div>
      </form>

      {result?.interpretedQuery.length ? (
        <div className="interpreted-query" aria-label="Requête interprétée">
          <span>Compris localement :</span>
          {result.interpretedQuery.map((chip) => (
            <button
              key={chip.id}
              type="button"
              className="query-chip"
              aria-label={`Retirer ${chip.label}`}
              onClick={() => {
                setPage(0);
                setDisabledIntents((current) =>
                  current.includes(chip.kind) ? current : [...current, chip.kind],
                );
              }}
            >
              {chip.label} <span aria-hidden="true">×</span>
            </button>
          ))}
          <small>Retirez une interprétation pour la rechercher comme texte libre.</small>
        </div>
      ) : null}

      <div className="search-controls" aria-label="Filtres de recherche">
        <Filter label="Type de document">
          <select
            value={filters.documentType}
            onChange={(event) =>
              updateFilter("documentType", event.target.value as SearchDocumentType)
            }
          >
            <option value="all">Tous</option>
            <option value="invoice">Factures</option>
            <option value="quote">Devis</option>
            <option value="contract">Contrats</option>
            <option value="administrative_document">Administratif</option>
            <option value="photo">Photos</option>
            <option value="spreadsheet">Tableurs</option>
          </select>
        </Filter>
        <Filter label="Contexte">
          <select
            value={filters.context}
            onChange={(event) =>
              updateFilter("context", event.target.value as SearchContext)
            }
          >
            <option value="any">Tous</option>
            <option value="personal">Personnel</option>
            <option value="business">Professionnel</option>
            <option value="mixed">Mixte</option>
            <option value="unknown">À déterminer</option>
          </select>
        </Filter>
        <Filter label="Trier">
          <select
            value={sort}
            onChange={(event) => {
              setPage(0);
              setSort(event.target.value as SearchSort);
            }}
          >
            <option value="relevance">Pertinence</option>
            <option value="newest">Plus récents</option>
            <option value="oldest">Plus anciens</option>
            <option value="filename">Nom</option>
            <option value="size">Taille</option>
          </select>
        </Filter>
      </div>

      <details className="advanced-search">
        <summary>Filtres structurés</summary>
        <div className="advanced-search-grid">
          <Filter label="Fournisseur">
            <input
              value={filters.supplier ?? ""}
              maxLength={128}
              placeholder="Point P"
              onChange={(event) => updateFilter("supplier", event.target.value || null)}
            />
          </Filter>
          <Filter label="Client">
            <input
              value={filters.customer ?? ""}
              maxLength={128}
              placeholder="Dupont"
              onChange={(event) => updateFilter("customer", event.target.value || null)}
            />
          </Filter>
          <Filter label="Projet / chantier">
            <input
              value={filters.project ?? ""}
              maxLength={128}
              placeholder="Martin"
              onChange={(event) => updateFilter("project", event.target.value || null)}
            />
          </Filter>
          <Filter label="Année">
            <input
              type="number"
              min={1900}
              max={2100}
              value={filters.year ?? ""}
              onChange={(event) =>
                updateFilter("year", event.target.value ? Number(event.target.value) : null)
              }
            />
          </Filter>
          <Filter label="Montant min. (€)">
            <input
              type="number"
              min={0}
              step="0.01"
              value={minorToInput(filters.amountMinimumMinor)}
              onChange={(event) =>
                updateFilter("amountMinimumMinor", amountToMinor(event.target.value))
              }
            />
          </Filter>
          <Filter label="Montant max. (€)">
            <input
              type="number"
              min={0}
              step="0.01"
              value={minorToInput(filters.amountMaximumMinor)}
              onChange={(event) =>
                updateFilter("amountMaximumMinor", amountToMinor(event.target.value))
              }
            />
          </Filter>
          <Filter label="Devise">
            <select
              value={filters.currency ?? ""}
              onChange={(event) => updateFilter("currency", event.target.value || null)}
            >
              <option value="">Toutes</option>
              <option value="EUR">EUR</option>
              <option value="USD">USD</option>
              <option value="GBP">GBP</option>
            </select>
          </Filter>
          <Filter label="État sémantique">
            <select
              value={filters.semanticStatus}
              onChange={(event) =>
                updateFilter("semanticStatus", event.target.value as SearchSemanticStatus)
              }
            >
              <option value="any">Tous</option>
              <option value="success">Analysé</option>
              <option value="partial">Partiel</option>
              <option value="unknown">Inconnu</option>
              <option value="failed">Échec</option>
              <option value="pending">En attente</option>
            </select>
          </Filter>
          <Filter label="Confiance min.">
            <select
              value={filters.minimumConfidencePercent ?? ""}
              onChange={(event) =>
                updateFilter(
                  "minimumConfidencePercent",
                  event.target.value ? Number(event.target.value) : null,
                )
              }
            >
              <option value="">Toutes</option>
              <option value="65">65 %</option>
              <option value="85">85 %</option>
              <option value="95">95 %</option>
            </select>
          </Filter>
          <Filter label="Format">
            <select
              value={filters.fileType}
              onChange={(event) =>
                updateFilter("fileType", event.target.value as SearchFileType)
              }
            >
              <option value="all">Tous</option>
              <option value="pdf">PDF</option>
              <option value="documents">Documents</option>
              <option value="spreadsheets">Tableurs</option>
              <option value="presentations">Présentations</option>
              <option value="images">Images</option>
              <option value="archives">Archives</option>
              <option value="other">Autres</option>
            </select>
          </Filter>
          <Filter label="Date de modification">
            <select
              value={filters.modified}
              onChange={(event) =>
                updateFilter("modified", event.target.value as SearchModified)
              }
            >
              <option value="any">N’importe quand</option>
              <option value="today">Aujourd’hui</option>
              <option value="last_7_days">7 derniers jours</option>
              <option value="last_30_days">30 derniers jours</option>
              <option value="this_year">Cette année</option>
            </select>
          </Filter>
          <Filter label="Extraction">
            <select
              value={filters.extraction}
              onChange={(event) =>
                updateFilter("extraction", event.target.value as SearchExtraction)
              }
            >
              <option value="any">Tous les états</option>
              <option value="success">Réussie</option>
              <option value="partial">Partielle</option>
              <option value="failed">Échec</option>
              <option value="unsupported">Non prise en charge</option>
            </select>
          </Filter>
          <Filter label="OCR">
            <select
              value={filters.ocr}
              onChange={(event) => updateFilter("ocr", event.target.value as SearchOcr)}
            >
              <option value="any">Tous</option>
              <option value="used">Utilisé</option>
              <option value="not_used">Non utilisé</option>
              <option value="unavailable">Indisponible</option>
            </select>
          </Filter>
        </div>
      </details>

      <div className="results-count" aria-live="polite">
        <strong>{result?.total.toLocaleString() ?? "—"}</strong>
        <span>{result?.total === 1 ? "résultat" : "résultats"}</span>
        {loading ? <span className="loading-label">Recherche…</span> : null}
        {!loading && result ? (
          <span className="search-mode">
            {result.embeddings.availability === "available_production" ||
            result.embeddings.availability === "available_development"
              ? "Recherche intelligente active"
              : "Recherche classique"}
          </span>
        ) : null}
      </div>

      {error ? (
        <div className="notice-banner notice-banner--warning" role="status">
          <div>
            <strong>{error.title}</strong>
            <span>{error.message}</span>
            <span className="notice-banner__impact">{error.impact}</span>
            <span className="notice-banner__hint">{error.actionHint}</span>
          </div>
          <button type="button" onClick={() => setError(null)}>
            Continuer
          </button>
        </div>
      ) : null}
      {!loading && !error && result?.results.length === 0 ? (
        <div className="empty-state">
          <strong>Aucun fichier trouvé</strong>
          <p>
            Commencez par analyser vos fichiers pour pouvoir les retrouver ici,
            ou essayez d’autres mots.
          </p>
        </div>
      ) : null}

      <div className="search-results" aria-busy={loading}>
        {result?.results.map((item) => (
          <article className="search-result" key={item.fileId}>
            <div className="file-kind" aria-hidden="true">
              {fileIcon(item.extension)}
            </div>
            <div className="search-result-body">
              <div className="search-result-title">
                <div>
                  <h3>{item.filename}</h3>
                  <code>{item.relativePath}</code>
                </div>
                <button type="button" onClick={() => onOpenFile(item.fileId)}>
                  Voir les détails
                </button>
              </div>
              {item.snippet ? <p className="result-snippet">{item.snippet}</p> : null}
              {item.whyMatched.length ? (
                <div className="why-matched">
                  <strong>Pourquoi ce résultat ?</strong>
                  <ul>
                    {item.whyMatched.map((reason) => (
                      <li key={reason}>{reason}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              <div className="result-meta">
                <span>{item.detectedType ?? item.extension ?? "Type inconnu"}</span>
                <span>{formatBytes(item.byteSize)}</span>
                <span>{formatTimestamp(item.modifiedAt)}</span>
                <span>{sourceLabel(item.matchSource)}</span>
                {item.extractionStatus ? (
                  <span className={`state-dot state-dot--${item.extractionStatus}`}>
                    {statusLabel(item.extractionStatus)}
                  </span>
                ) : null}
              </div>
            </div>
          </article>
        ))}
      </div>

      {result && (page > 0 || result.hasMore) ? (
        <nav className="pagination" aria-label="Pages de résultats">
          <button
            type="button"
            disabled={page === 0 || loading}
            onClick={() => setPage((current) => Math.max(0, current - 1))}
          >
            Précédent
          </button>
          <span>Page {page + 1}</span>
          <button
            type="button"
            disabled={!result.hasMore || loading}
            onClick={() => setPage((current) => current + 1)}
          >
            Suivant
          </button>
        </nav>
      ) : null}
    </div>
  );
}

function Filter({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label>
      <span>{label}</span>
      {children}
    </label>
  );
}

function fileIcon(extension?: string | null): string {
  const value = extension?.toLowerCase();
  if (value === "pdf") return "PDF";
  if (["png", "jpg", "jpeg", "webp", "tiff", "bmp"].includes(value ?? "")) return "IMG";
  if (["xls", "xlsx", "csv"].includes(value ?? "")) return "XLS";
  if (["ppt", "pptx"].includes(value ?? "")) return "PPT";
  return "DOC";
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatTimestamp(value?: string | null): string {
  if (!value) return "Date inconnue";
  try {
    const date = /^\d+$/.test(value)
      ? new Date(Number(BigInt(value) / 1_000_000n))
      : new Date(value);
    return Number.isNaN(date.getTime()) ? "Date inconnue" : date.toLocaleDateString();
  } catch {
    return "Date inconnue";
  }
}

function amountToMinor(value: string): number | null {
  if (!value.trim()) return null;
  const amount = Number(value);
  return Number.isFinite(amount) && amount >= 0 ? Math.round(amount * 100) : null;
}

function minorToInput(value?: number | null): string {
  return typeof value === "number" && Number.isFinite(value) ? String(value / 100) : "";
}

function sourceLabel(source: string): string {
  const labels: Record<string, string> = {
    filename: "Nom correspondant",
    path: "Emplacement correspondant",
    content: "Dans le contenu",
    metadata: "Type correspondant",
    structured: "Données structurées",
    relationship: "Relation confirmée ou probable",
    semantic: "Similarité locale",
  };
  return labels[source] ?? "Correspondance locale";
}

function statusLabel(status: string): string {
  const labels: Record<string, string> = {
    success: "Texte extrait",
    partial: "Extraction partielle",
    failed: "Extraction impossible",
    unsupported: "Format non pris en charge",
  };
  return labels[status] ?? status;
}
