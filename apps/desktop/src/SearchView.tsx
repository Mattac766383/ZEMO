import { useEffect, useMemo, useState } from "react";
import {
  activateLocalEmbeddingModel,
  cancelLocalEmbeddingModelInstall,
  getEmbeddingModelStatus,
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
  SearchSort,
} from "./types";
import "./SearchViewV2.css";

const PAGE_SIZE = 30;
const INITIAL_FILTERS: LocalSearchFilters = {
  fileType: "all",
  modified: "any",
  extraction: "any",
  ocr: "any",
  documentType: "any",
  context: "any",
  semanticStatus: "any",
};

const SEARCH_EXAMPLES = [
  "la facture Point P du chantier Martin autour de 1 400 €",
  "le devis de Dupont de l'année dernière",
  "la photo de la toiture prise cet été",
];

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
  const [modelStatus, setModelStatus] = useState<EmbeddingModelStatus | null>(null);
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

  function updateText(value: string) {
    setPage(0);
    setDisabledIntents([]);
    setText(value);
  }

  async function runModelAction(
    action: () => Promise<EmbeddingModelStatus>,
  ) {
    setModelBusy(true);
    setModelMessage(null);
    try {
      const status = await action();
      setModelStatus(status);
      if (status.status === "ready") {
        setModelMessage(
          "L’intelligence sémantique locale est prête. ZEMO peut chercher par sens, contenu et contexte.",
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

  const semanticReady =
    modelStatus?.status === "ready" || result?.embeddings?.productionReady === true;
  const semanticInstalling =
    modelStatus?.status === "downloading" ||
    modelStatus?.status === "installing" ||
    modelStatus?.status === "loading";
  const hasVisibleQuery =
    text.trim().length > 0 || Boolean(result?.query?.trim().length);
  const activeFilterCount = useMemo(
    () => countActiveFilters(filters, sort),
    [filters, sort],
  );

  return (
    <main className="search-v2" aria-labelledby="search-v2-title">
      <header className="search-v2__header">
        <span className="step">Recherche</span>
        <h2 id="search-v2-title">Décrivez simplement ce que vous cherchez</h2>
        <p>
          Pas besoin de connaître le nom du fichier. ZEMO cherche dans le nom, le
          contenu, les dates, les montants, les personnes, les entreprises et les
          projets compris localement.
        </p>
        <span className={semanticReady ? "search-v2__brain search-v2__brain--ready" : "search-v2__brain"}>
          {semanticReady ? "Intelligence sémantique locale active" : "Recherche classique active"}
        </span>
      </header>

      <form
        className="search-v2__form"
        role="search"
        onSubmit={(event) => event.preventDefault()}
      >
        <label htmlFor="local-search-input">Recherche</label>
        <div className="search-v2__input-wrap">
          <span aria-hidden="true">⌕</span>
          <input
            id="local-search-input"
            type="search"
            maxLength={512}
            autoComplete="off"
            autoFocus
            placeholder="Ex. la facture Point P du chantier Martin autour de 1 400 €"
            value={text}
            onChange={(event) => updateText(event.target.value)}
          />
          {text ? (
            <button
              type="button"
              className="search-v2__clear"
              aria-label="Effacer la recherche"
              onClick={() => updateText("")}
            >
              ×
            </button>
          ) : null}
        </div>
      </form>

      {!text.trim() ? (
        <div className="search-v2__examples" aria-label="Exemples de recherche">
          <span>Essayez par exemple :</span>
          {SEARCH_EXAMPLES.map((example) => (
            <button type="button" key={example} onClick={() => updateText(example)}>
              {example}
            </button>
          ))}
        </div>
      ) : null}

      {!semanticReady ? (
        <section className="search-v2__intelligence" aria-label="Améliorer la recherche">
          <div>
            <strong>Recherche intelligente</strong>
            <p>
              {semanticInstalling
                ? "Activation de l’intelligence locale en cours…"
                : "Recherche améliorée non activée. La recherche classique reste disponible."}
            </p>
            <small>
              Le modèle fonctionne sur cet appareil. Environ {formatMegabytes(modelStatus?.approximateDiskBytes)} à télécharger une seule fois.
            </small>
            {modelMessage ? <p className="inline-error">{modelMessage}</p> : null}
          </div>
          <div className="search-v2__intelligence-actions">
            {semanticInstalling ? (
              <button
                type="button"
                disabled={modelBusy}
                onClick={() => void runModelAction(cancelLocalEmbeddingModelInstall)}
              >
                Annuler
              </button>
            ) : modelStatus?.status === "failed" ||
              modelStatus?.status === "corrupt" ||
              modelStatus?.status === "incompatible_version" ? (
              <button
                type="button"
                disabled={modelBusy}
                onClick={() => void runModelAction(retryLocalEmbeddingModel)}
              >
                Réessayer
              </button>
            ) : (
              <button
                type="button"
                className="primary-action"
                disabled={modelBusy}
                onClick={() => void runModelAction(activateLocalEmbeddingModel)}
              >
                Activer
              </button>
            )}
          </div>
        </section>
      ) : modelMessage ? (
        <p className="notice-banner" role="status">{modelMessage}</p>
      ) : null}

      {result?.interpretedQuery?.length ? (
        <div className="search-v2__understood" aria-label="Requête interprétée">
          <span>ZEMO a compris :</span>
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
        </div>
      ) : null}

      <details className="search-v2__filters">
        <summary>
          Filtres structurés{activeFilterCount > 0 ? ` · ${activeFilterCount} actif${activeFilterCount > 1 ? "s" : ""}` : ""}
        </summary>
        <p className="search-v2__filters-help">
          Facultatif : la phrase de recherche suffit dans la majorité des cas.
        </p>
        <div className="search-v2__filters-grid">
          <Filter label="Type de document">
            <select
              value={filters.documentType}
              onChange={(event) =>
                updateFilter("documentType", event.target.value as SearchDocumentType)
              }
            >
              <option value="any">Tous</option>
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
        </div>
        <div className="search-v2__filter-actions">
          <button
            type="button"
            onClick={() => {
              setFilters(INITIAL_FILTERS);
              setSort("relevance");
              setPage(0);
            }}
          >
            Réinitialiser les filtres
          </button>
          {semanticReady ? (
            <button
              type="button"
              disabled={modelBusy}
              onClick={() => void runModelAction(removeLocalEmbeddingModel)}
            >
              Désactiver l’intelligence locale
            </button>
          ) : null}
        </div>
      </details>

      {error ? (
        <p className="inline-error" role="alert">{error.message}</p>
      ) : null}

      {loading ? (
        <p className="search-v2__loading" role="status">ZEMO cherche sur cet appareil…</p>
      ) : null}

      {!loading && !error && hasVisibleQuery && result ? (
        <section className="search-v2__results" aria-label="Résultats de recherche">
          <div className="search-v2__results-heading">
            <div>
              <strong>
                {result.total.toLocaleString()} résultat{result.total === 1 ? "" : "s"}
              </strong>
              <span>
                {semanticReady
                  ? "Recherche hybride : sens + contenu + contexte + nom"
                  : "Recherche classique : nom + contenu + métadonnées"}
              </span>
            </div>
          </div>

          {result.results.length === 0 ? (
            <div className="empty-state">
              <strong>Aucun fichier trouvé</strong>
              <p>Essayez de décrire le document autrement, avec une personne, une date, un projet ou un montant.</p>
            </div>
          ) : (
            <div className="search-v2__result-list">
              {result.results.map((item) => (
                <article className="search-v2__result" key={item.fileId}>
                  <div className="search-v2__result-top">
                    <div className="search-v2__file-icon" aria-hidden="true">
                      {fileIcon(item.extension, item.detectedType)}
                    </div>
                    <div className="search-v2__result-title">
                      <h3>{item.filename}</h3>
                      <div className="search-v2__meta">
                        <span>{fileTypeLabel(item.extension, item.detectedType)}</span>
                        <span>{formatBytes(item.byteSize)}</span>
                        {item.modifiedAt ? <span>Modifié {formatTimestamp(item.modifiedAt)}</span> : null}
                        <span>{sourceLabel(item.matchSource)}</span>
                      </div>
                    </div>
                    <button
                      type="button"
                      className="primary-action search-v2__details-button"
                      onClick={() => onOpenFile(item.fileId)}
                    >
                      Voir les détails
                    </button>
                  </div>

                  {item.snippet ? <p className="search-v2__snippet">{item.snippet}</p> : null}

                  <PathTree relativePath={item.relativePath} filename={item.filename} />

                  {item.whyMatched.length ? (
                    <details className="search-v2__why">
                      <summary>Pourquoi ce résultat ?</summary>
                      <ul>
                        {item.whyMatched.map((reason, index) => (
                          <li key={`${item.fileId}-reason-${index}`}>{reason}</li>
                        ))}
                      </ul>
                    </details>
                  ) : null}
                </article>
              ))}
            </div>
          )}

          {result.total > PAGE_SIZE ? (
            <nav className="pagination" aria-label="Pages des résultats">
              <button
                type="button"
                disabled={page === 0 || loading}
                onClick={() => setPage((value) => Math.max(0, value - 1))}
              >
                Précédent
              </button>
              <span>Page {page + 1}</span>
              <button
                type="button"
                disabled={!result.hasMore || loading}
                onClick={() => setPage((value) => value + 1)}
              >
                Suivant
              </button>
            </nav>
          ) : null}
        </section>
      ) : null}
    </main>
  );
}

function PathTree({ relativePath, filename }: { relativePath: string; filename: string }) {
  const parts = splitPath(relativePath);
  const visibleParts = parts.length > 0 ? parts : [filename];
  return (
    <div className="search-v2__path" aria-label={`Emplacement de ${filename}`}>
      <span className="search-v2__path-label">Emplacement</span>
      <div className="search-v2__path-tree">
        {visibleParts.map((part, index) => {
          const isFile = index === visibleParts.length - 1;
          return (
            <div
              key={`${part}-${index}`}
              className={isFile ? "search-v2__path-part search-v2__path-part--file" : "search-v2__path-part"}
              style={{ paddingInlineStart: `${index * 18}px` }}
            >
              <span aria-hidden="true">{index === 0 ? "" : "└─"}</span>
              <span aria-hidden="true">{isFile ? "▧" : "▸"}</span>
              <span>{part}</span>
            </div>
          );
        })}
      </div>
      <code>{relativePath || filename}</code>
    </div>
  );
}

function Filter({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="search-v2__filter">
      <span>{label}</span>
      {children}
    </label>
  );
}

function splitPath(value: string): string[] {
  return value
    .split(/[\\/]+/g)
    .map((part) => part.trim())
    .filter(Boolean);
}

function countActiveFilters(filters: LocalSearchFilters, sort: SearchSort): number {
  let count = sort === "relevance" ? 0 : 1;
  if (filters.documentType !== "any") count += 1;
  if (filters.context !== "any") count += 1;
  if (filters.supplier) count += 1;
  if (filters.customer) count += 1;
  if (filters.project) count += 1;
  if (filters.year) count += 1;
  if (filters.amountMinimumMinor != null) count += 1;
  if (filters.amountMaximumMinor != null) count += 1;
  return count;
}

function amountToMinor(value: string): number | null {
  if (!value.trim()) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.round(parsed * 100) : null;
}

function minorToInput(value?: number | null): string {
  return value == null ? "" : String(value / 100);
}

function formatMegabytes(bytes?: number | null): string {
  if (!bytes || bytes <= 0) return "118 Mo";
  return `${Math.round(bytes / (1024 * 1024))} Mo`;
}

function fileIcon(extension?: string | null, detectedType?: string | null): string {
  const ext = extension?.toLowerCase();
  if (detectedType?.startsWith("image/") || ["jpg", "jpeg", "png", "heic", "webp"].includes(ext ?? "")) return "▧";
  if (ext === "pdf") return "PDF";
  if (["xls", "xlsx", "csv", "ods"].includes(ext ?? "")) return "▦";
  if (["doc", "docx", "odt", "rtf"].includes(ext ?? "")) return "▤";
  return "▣";
}

function fileTypeLabel(extension?: string | null, detectedType?: string | null): string {
  if (extension) return extension.toUpperCase();
  if (detectedType) return detectedType;
  return "Fichier";
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const amount = value / 1024 ** index;
  return `${amount.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatTimestamp(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString();
}

function sourceLabel(source: string): string {
  const labels: Record<string, string> = {
    filename: "Nom correspondant",
    path: "Emplacement correspondant",
    content: "Contenu correspondant",
    metadata: "Métadonnées correspondantes",
    structured: "Informations comprises",
    relationship: "Contexte relié",
    semantic: "Sens correspondant",
  };
  return labels[source] ?? "Correspondance";
}
