import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { getErrorMessage, getFileDetail, probeUserContentAccess, searchLocalFiles } from "./api";
import { FileDetailPanel } from "./FileDetailPanel";
import { IdentityDetailPanel } from "./IdentityDetailPanel";
import type { FolderAccessProbe, LocalFileDetail, LocalSearchResult } from "./types";
import {
  buildKnowledgeMapModel,
  type KnowledgeContext,
  type KnowledgeMapEdge,
  type KnowledgeMapInput,
  type KnowledgeMapNode,
  type KnowledgeNodeKind,
} from "./knowledgeMapModel";
import "./KnowledgeMapView.css";

const DETAIL_LIMIT = 120;
const EXPANDED_FILE_LIMIT = 24;
const DETAIL_CONCURRENCY = 6;
const CANVAS_WIDTH = 1400;
const CANVAS_HEIGHT = 920;

type ContextFilter = "all" | KnowledgeContext | "review";
type TypeFilter = "all" | Exclude<KnowledgeNodeKind, "root" | "context">;
type RenderKind = KnowledgeNodeKind | "file";

interface KnowledgeMapViewProps {
  workspaceId: string;
  onClose: () => void;
}

interface RenderNode {
  id: string;
  kind: RenderKind;
  label: string;
  fileCount: number;
  reviewCount: number;
  contexts: KnowledgeContext[];
  fileIds: string[];
  identityId?: string | null;
  relationshipKind?: string | null;
  confidence?: number | null;
  fileId?: string | null;
}

interface PositionedNode extends RenderNode {
  x: number;
  y: number;
}

export function KnowledgeMapView({ workspaceId, onClose }: KnowledgeMapViewProps) {
  const [inputs, setInputs] = useState<KnowledgeMapInput[]>([]);
  const [totalFiles, setTotalFiles] = useState(0);
  const [coverage, setCoverage] = useState<FolderAccessProbe[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [contextFilter, setContextFilter] = useState<ContextFilter>("all");
  const [typeFilter, setTypeFilter] = useState<TypeFilter>("all");
  const [searchTerm, setSearchTerm] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [panelFileId, setPanelFileId] = useState<string | null>(null);
  const [panelIdentityId, setPanelIdentityId] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    originX: number;
    originY: number;
  } | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    setPanelFileId(null);
    setPanelIdentityId(null);
    setSelectedNodeId(null);

    void Promise.all([
      searchLocalFiles(workspaceId, {
        text: "",
        filters: {
          fileType: "all",
          modified: "any",
          extraction: "any",
          ocr: "any",
          documentType: "any",
          context: "any",
          semanticStatus: "any",
        },
        sort: "newest",
        page: 0,
        pageSize: DETAIL_LIMIT,
        semanticSearch: false,
        disabledIntents: [],
      }),
      probeUserContentAccess().catch(() => [] as FolderAccessProbe[]),
    ])
      .then(async ([page, probes]) => {
        if (!active) {
          return;
        }
        const boundedResults = page.results.slice(0, DETAIL_LIMIT);
        const details = await loadDetailsBounded(boundedResults, () => active);
        if (!active) {
          return;
        }
        setTotalFiles(page.total);
        setCoverage(probes);
        setInputs(
          boundedResults.map((result, index) => ({
            result,
            detail: details[index] ?? null,
          })),
        );
      })
      .catch((reason) => {
        if (active) {
          setError(getErrorMessage(reason));
          setInputs([]);
          setTotalFiles(0);
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
  }, [reloadKey, workspaceId]);

  const model = useMemo(() => buildKnowledgeMapModel(inputs), [inputs]);
  const visible = useMemo(
    () => buildVisibleGraph(model.nodes, model.edges, model.files, expanded, contextFilter, typeFilter),
    [contextFilter, expanded, model.edges, model.files, model.nodes, typeFilter],
  );
  const positioned = useMemo(() => layoutNodes(visible.nodes), [visible.nodes]);
  const positions = useMemo(
    () => new Map(positioned.map((node) => [node.id, { x: node.x, y: node.y }])),
    [positioned],
  );
  const selectedNode = visible.nodes.find((node) => node.id === selectedNodeId) ?? null;

  function resetView() {
    setZoom(1);
    setPan({ x: 0, y: 0 });
  }

  function showEverything() {
    setContextFilter("all");
    setTypeFilter("all");
    setSearchTerm("");
    setExpanded(new Set());
    setSelectedNodeId(null);
    resetView();
  }

  function selectNode(node: RenderNode) {
    setSelectedNodeId(node.id);
    setPanelFileId(node.fileId ?? null);
    setPanelIdentityId(null);
  }

  function toggleExpanded(nodeId: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(nodeId)) {
        next.delete(nodeId);
      } else {
        next.add(nodeId);
      }
      return next;
    });
  }

  function onPointerDown(event: ReactPointerEvent<SVGSVGElement>) {
    if (event.target !== event.currentTarget) {
      return;
    }
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      originX: pan.x,
      originY: pan.y,
    };
  }

  function onPointerMove(event: ReactPointerEvent<SVGSVGElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) {
      return;
    }
    setPan({
      x: drag.originX + (event.clientX - drag.startX) / zoom,
      y: drag.originY + (event.clientY - drag.startY) / zoom,
    });
  }

  function onPointerUp(event: ReactPointerEvent<SVGSVGElement>) {
    if (dragRef.current?.pointerId === event.pointerId) {
      dragRef.current = null;
    }
  }

  const projects = model.nodes.filter((node) => node.kind === "project").length;
  const identities = model.nodes.filter(
    (node) => node.kind === "organization" || node.kind === "person",
  ).length;
  const isPartial = totalFiles > model.detailedFiles;

  return (
    <section className="knowledge-map-shell" aria-labelledby="knowledge-map-title">
      <header className="knowledge-map-header">
        <div>
          <span className="step">Exploration locale · lecture seule</span>
          <h2 id="knowledge-map-title">Carte ZEMO</h2>
          <p>
            Visualisez ce que ZEMO comprend de vos fichiers, projets, personnes et organisations.
          </p>
        </div>
        <button type="button" onClick={onClose} aria-label="Fermer la carte ZEMO">
          Retour
        </button>
      </header>

      {error ? (
        <div className="knowledge-map-error" role="alert">
          <strong>La carte n’a pas pu être chargée.</strong>
          <span>{error}</span>
          <button type="button" onClick={() => setReloadKey((value) => value + 1)}>
            Réessayer
          </button>
        </div>
      ) : null}

      {loading ? (
        <div className="knowledge-map-loading" aria-live="polite">
          <div className="knowledge-map-spinner" />
          <strong>ZEMO construit votre carte locale…</strong>
          <span>Aucun fichier n’est déplacé et aucune donnée n’est envoyée en ligne.</span>
        </div>
      ) : null}

      {!loading && !error && totalFiles === 0 ? (
        <div className="knowledge-map-empty">
          <div className="knowledge-map-empty-icon" aria-hidden="true">◎</div>
          <h3>Votre carte apparaîtra ici</h3>
          <p>
            Analysez vos fichiers pour que ZEMO puisse comprendre les projets, personnes,
            entreprises et documents qui composent votre ordinateur.
          </p>
          <button type="button" className="primary" onClick={onClose}>
            Retour pour analyser mes fichiers
          </button>
        </div>
      ) : null}

      {!loading && !error && totalFiles > 0 ? (
        <>
          <div className="knowledge-map-stats" aria-label="Résumé de la carte">
            <Stat value={totalFiles} label="fichiers indexés" />
            <Stat value={model.detailedFiles} label="fichiers détaillés dans cette vue" />
            <Stat value={projects} label="projets détectés" />
            <Stat value={identities} label="personnes / organisations" />
            <Stat value={model.needsReview} label="à vérifier" attention={model.needsReview > 0} />
          </div>

          {isPartial ? (
            <p className="knowledge-map-partial" role="status">
              Carte progressive basée sur {model.detailedFiles.toLocaleString()} fichiers détaillés
              sur {totalFiles.toLocaleString()} indexés. Les fichiers individuels restent chargés à
              la demande pour préserver la fluidité.
            </p>
          ) : (
            <p className="knowledge-map-partial knowledge-map-partial--complete" role="status">
              Carte basée sur les {totalFiles.toLocaleString()} fichiers indexés disponibles dans ce corpus.
            </p>
          )}

          <div className="knowledge-map-toolbar">
            <label className="knowledge-map-search">
              <span>Rechercher dans la carte</span>
              <input
                value={searchTerm}
                onChange={(event) => setSearchTerm(event.target.value)}
                placeholder="Martin, Point P, 2026, factures…"
              />
            </label>
            <div className="knowledge-map-filter-row" aria-label="Filtres de contexte">
              {([
                ["all", "Tout"],
                ["business", "Pro"],
                ["personal", "Perso"],
                ["review", "À vérifier"],
              ] as Array<[ContextFilter, string]>).map(([value, label]) => (
                <button
                  type="button"
                  key={value}
                  className={contextFilter === value ? "is-active" : undefined}
                  onClick={() => setContextFilter(value)}
                >
                  {label}
                </button>
              ))}
            </div>
            <label className="knowledge-map-type-filter">
              <span>Afficher</span>
              <select
                value={typeFilter}
                onChange={(event) => setTypeFilter(event.target.value as TypeFilter)}
              >
                <option value="all">Tous les liens</option>
                <option value="project">Projets</option>
                <option value="organization">Organisations</option>
                <option value="person">Personnes</option>
                <option value="document_type">Types de documents</option>
                <option value="year">Années</option>
              </select>
            </label>
            <div className="knowledge-map-zoom" aria-label="Contrôles de carte">
              <button type="button" onClick={() => setZoom((value) => Math.max(0.55, value - 0.1))}>
                −
              </button>
              <span>{Math.round(zoom * 100)} %</span>
              <button type="button" onClick={() => setZoom((value) => Math.min(1.8, value + 0.1))}>
                +
              </button>
              <button type="button" onClick={resetView}>Recentrer</button>
              <button type="button" onClick={showEverything}>Tout afficher</button>
            </div>
          </div>

          <div className="knowledge-map-main">
            <div className="knowledge-map-canvas-wrap">
              <svg
                className="knowledge-map-canvas"
                viewBox={`0 0 ${CANVAS_WIDTH} ${CANVAS_HEIGHT}`}
                role="img"
                aria-label="Carte interactive des relations entre les fichiers"
                onPointerDown={onPointerDown}
                onPointerMove={onPointerMove}
                onPointerUp={onPointerUp}
                onPointerCancel={onPointerUp}
              >
                <g transform={`translate(${pan.x} ${pan.y}) scale(${zoom})`}>
                  {visible.edges.map((edge) => {
                    const source = positions.get(edge.sourceId);
                    const target = positions.get(edge.targetId);
                    if (!source || !target) {
                      return null;
                    }
                    return (
                      <line
                        key={edge.id}
                        className="knowledge-map-edge"
                        x1={source.x}
                        y1={source.y}
                        x2={target.x}
                        y2={target.y}
                      />
                    );
                  })}
                  {positioned.map((node) => {
                    const selected = selectedNodeId === node.id;
                    const matched = nodeMatchesSearch(node, searchTerm, model.files);
                    const dimmed = searchTerm.trim().length > 0 && !matched;
                    return (
                      <g
                        key={node.id}
                        transform={`translate(${node.x} ${node.y})`}
                        className={`knowledge-map-node knowledge-map-node--${node.kind}${selected ? " is-selected" : ""}${matched && searchTerm.trim() ? " is-match" : ""}${dimmed ? " is-dimmed" : ""}`}
                        role="button"
                        tabIndex={0}
                        aria-label={`${kindLabel(node.kind)} ${node.label}, ${node.fileCount} fichier${node.fileCount === 1 ? "" : "s"}`}
                        onClick={() => selectNode(node)}
                        onDoubleClick={() => {
                          if (node.kind !== "root" && node.kind !== "context" && node.kind !== "file") {
                            toggleExpanded(node.id);
                          }
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            selectNode(node);
                          }
                          if (
                            event.key === " " &&
                            node.kind !== "root" &&
                            node.kind !== "context" &&
                            node.kind !== "file"
                          ) {
                            event.preventDefault();
                            toggleExpanded(node.id);
                          }
                        }}
                      >
                        <rect x={-82} y={-30} width={164} height={60} rx={18} />
                        <text className="knowledge-map-node-icon" x={-63} y={5}>
                          {kindIcon(node.kind)}
                        </text>
                        <text className="knowledge-map-node-label" x={-38} y={-3}>
                          {shorten(node.label, 20)}
                        </text>
                        <text className="knowledge-map-node-count" x={-38} y={16}>
                          {node.fileCount} fichier{node.fileCount === 1 ? "" : "s"}
                          {node.reviewCount > 0 ? ` · ${node.reviewCount} à vérifier` : ""}
                        </text>
                      </g>
                    );
                  })}
                </g>
              </svg>
              <p className="knowledge-map-canvas-hint">
                Glissez le fond pour déplacer la carte. Double-cliquez un groupe pour révéler ses fichiers.
              </p>
            </div>

            <aside className="knowledge-map-inspector" aria-label="Détail de la sélection">
              {panelFileId ? (
                <FileDetailPanel
                  fileId={panelFileId}
                  onClose={() => setPanelFileId(null)}
                  onOpenIdentity={(identityId) => {
                    setPanelFileId(null);
                    setPanelIdentityId(identityId);
                  }}
                />
              ) : panelIdentityId ? (
                <IdentityDetailPanel
                  identityId={panelIdentityId}
                  onClose={() => setPanelIdentityId(null)}
                  onOpenFile={(fileId) => {
                    setPanelIdentityId(null);
                    setPanelFileId(fileId);
                  }}
                  onOpenIdentity={setPanelIdentityId}
                />
              ) : selectedNode ? (
                <NodeInspector
                  node={selectedNode}
                  expanded={expanded.has(selectedNode.id)}
                  files={model.files}
                  onToggle={() => toggleExpanded(selectedNode.id)}
                  onOpenFile={setPanelFileId}
                  onOpenIdentity={(identityId) => setPanelIdentityId(identityId)}
                />
              ) : (
                <div className="knowledge-map-inspector-empty">
                  <span aria-hidden="true">↖</span>
                  <h3>Sélectionnez un élément</h3>
                  <p>
                    Cliquez sur un projet, une entreprise, une année ou un type de document pour
                    voir les fichiers qui le composent.
                  </p>
                </div>
              )}
            </aside>
          </div>

          <details className="knowledge-map-coverage">
            <summary>Sources analysées</summary>
            {coverage.length > 0 ? (
              <ul>
                {coverage.map((probe) => (
                  <li key={probe.kind}>
                    <strong>{probe.displayLabel}</strong>
                    <span className={`coverage-state coverage-state--${coverageTone(probe.accessState)}`}>
                      {coverageLabel(probe.accessState)}
                    </span>
                  </li>
                ))}
              </ul>
            ) : (
              <p>La couverture détaillée des dossiers n’est pas disponible dans cette session.</p>
            )}
          </details>

          <details className="knowledge-map-accessible-list">
            <summary>Alternative structurée de la carte ({visible.nodes.length} éléments)</summary>
            <ul>
              {visible.nodes
                .filter((node) => node.kind !== "root")
                .map((node) => (
                  <li key={`list-${node.id}`}>
                    <button type="button" onClick={() => selectNode(node)}>
                      <strong>{node.label}</strong>
                      <span>
                        {kindLabel(node.kind)} · {node.fileCount} fichier{node.fileCount === 1 ? "" : "s"}
                      </span>
                    </button>
                  </li>
                ))}
            </ul>
          </details>
        </>
      ) : null}
    </section>
  );
}

function NodeInspector({
  node,
  expanded,
  files,
  onToggle,
  onOpenFile,
  onOpenIdentity,
}: {
  node: RenderNode;
  expanded: boolean;
  files: ReturnType<typeof buildKnowledgeMapModel>["files"];
  onToggle: () => void;
  onOpenFile: (fileId: string) => void;
  onOpenIdentity: (identityId: string) => void;
}) {
  const sample = node.fileIds.slice(0, 10).map((fileId) => files[fileId]).filter(Boolean);
  return (
    <div className="knowledge-map-node-inspector">
      <span className="step">{kindLabel(node.kind)}</span>
      <h3>{node.label}</h3>
      <div className="knowledge-map-inspector-metrics">
        <span><strong>{node.fileCount}</strong> fichiers liés</span>
        <span><strong>{node.reviewCount}</strong> à vérifier</span>
        {node.confidence != null ? (
          <span><strong>{Math.round(node.confidence * 100)} %</strong> confiance max</span>
        ) : null}
      </div>
      {node.identityId ? (
        <button type="button" className="primary" onClick={() => onOpenIdentity(node.identityId!)}>
          Voir l’identité et les preuves
        </button>
      ) : null}
      {node.kind !== "root" && node.kind !== "context" && node.kind !== "file" ? (
        <button type="button" onClick={onToggle}>
          {expanded ? "Réduire les fichiers" : `Afficher les fichiers (${Math.min(node.fileCount, EXPANDED_FILE_LIMIT)})`}
        </button>
      ) : null}
      {sample.length > 0 ? (
        <div className="knowledge-map-file-sample">
          <h4>Fichiers associés</h4>
          {sample.map((file) => (
            <button type="button" key={file.fileId} onClick={() => onOpenFile(file.fileId)}>
              <strong>{file.filename}</strong>
              <span>{file.detectedType ?? file.extension ?? "Type inconnu"}</span>
            </button>
          ))}
          {node.fileCount > sample.length ? (
            <small>+ {node.fileCount - sample.length} autres fichiers dans ce groupe.</small>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function Stat({ value, label, attention = false }: { value: number; label: string; attention?: boolean }) {
  return (
    <div className={attention ? "knowledge-map-stat is-attention" : "knowledge-map-stat"}>
      <strong>{value.toLocaleString()}</strong>
      <span>{label}</span>
    </div>
  );
}

async function loadDetailsBounded(
  results: LocalSearchResult[],
  isActive: () => boolean,
): Promise<Array<LocalFileDetail | null>> {
  const output: Array<LocalFileDetail | null> = Array.from({ length: results.length }, () => null);
  let cursor = 0;
  async function worker() {
    while (isActive()) {
      const index = cursor;
      cursor += 1;
      if (index >= results.length) {
        return;
      }
      try {
        output[index] = await getFileDetail(results[index].fileId);
      } catch {
        output[index] = null;
      }
    }
  }
  await Promise.all(
    Array.from({ length: Math.min(DETAIL_CONCURRENCY, results.length) }, () => worker()),
  );
  return output;
}

function buildVisibleGraph(
  nodes: KnowledgeMapNode[],
  edges: KnowledgeMapEdge[],
  files: ReturnType<typeof buildKnowledgeMapModel>["files"],
  expanded: Set<string>,
  contextFilter: ContextFilter,
  typeFilter: TypeFilter,
): { nodes: RenderNode[]; edges: KnowledgeMapEdge[] } {
  const visibleNodes = new Map<string, RenderNode>();
  for (const node of nodes) {
    if (!nodeAllowed(node, contextFilter, typeFilter)) {
      continue;
    }
    visibleNodes.set(node.id, { ...node });
  }

  for (const node of nodes) {
    if (!expanded.has(node.id) || !visibleNodes.has(node.id)) {
      continue;
    }
    for (const fileId of node.fileIds.slice(0, EXPANDED_FILE_LIMIT)) {
      const file = files[fileId];
      if (!file) {
        continue;
      }
      const fileNodeId = `file:${fileId}`;
      if (!visibleNodes.has(fileNodeId)) {
        visibleNodes.set(fileNodeId, {
          id: fileNodeId,
          kind: "file",
          label: file.filename,
          fileCount: 1,
          reviewCount: file.needsReview ? 1 : 0,
          contexts: node.contexts,
          fileIds: [fileId],
          fileId,
        });
      }
    }
  }

  const outputEdges = edges.filter(
    (edge) => visibleNodes.has(edge.sourceId) && visibleNodes.has(edge.targetId),
  );
  for (const node of nodes) {
    if (!expanded.has(node.id) || !visibleNodes.has(node.id)) {
      continue;
    }
    for (const fileId of node.fileIds.slice(0, EXPANDED_FILE_LIMIT)) {
      const fileNodeId = `file:${fileId}`;
      if (visibleNodes.has(fileNodeId)) {
        outputEdges.push({
          id: `${node.id}->${fileNodeId}:file`,
          sourceId: node.id,
          targetId: fileNodeId,
          kind: "file",
        });
      }
    }
  }
  outputEdges.sort((left, right) => left.id.localeCompare(right.id));
  return { nodes: [...visibleNodes.values()], edges: outputEdges };
}

function nodeAllowed(node: KnowledgeMapNode, contextFilter: ContextFilter, typeFilter: TypeFilter) {
  if (node.kind === "root") {
    return true;
  }
  if (contextFilter === "review" && node.reviewCount === 0) {
    return false;
  }
  if (
    contextFilter !== "all" &&
    contextFilter !== "review" &&
    !node.contexts.includes(contextFilter)
  ) {
    return false;
  }
  if (node.kind === "context") {
    return typeFilter === "all";
  }
  return typeFilter === "all" || node.kind === typeFilter;
}

function layoutNodes(nodes: RenderNode[]): PositionedNode[] {
  const root = nodes.filter((node) => node.kind === "root");
  const contexts = nodes.filter((node) => node.kind === "context").sort(nodeSort);
  const groups = nodes
    .filter((node) => node.kind !== "root" && node.kind !== "context" && node.kind !== "file")
    .sort(nodeSort);
  const files = nodes.filter((node) => node.kind === "file").sort(nodeSort);
  return [
    ...root.map((node) => ({ ...node, x: CANVAS_WIDTH / 2, y: CANVAS_HEIGHT / 2 })),
    ...ring(contexts, 145, -Math.PI / 2),
    ...ring(groups, 315, -Math.PI / 2 + 0.12),
    ...ring(files, 500, -Math.PI / 2 + 0.06),
  ];
}

function ring(nodes: RenderNode[], radius: number, startAngle: number): PositionedNode[] {
  if (nodes.length === 0) {
    return [];
  }
  return nodes.map((node, index) => {
    const angle = startAngle + (index * Math.PI * 2) / nodes.length;
    return {
      ...node,
      x: CANVAS_WIDTH / 2 + Math.cos(angle) * radius,
      y: CANVAS_HEIGHT / 2 + Math.sin(angle) * radius,
    };
  });
}

function nodeSort(left: RenderNode, right: RenderNode) {
  return `${left.kind}:${left.label}:${left.id}`.localeCompare(
    `${right.kind}:${right.label}:${right.id}`,
    "fr",
    { sensitivity: "base" },
  );
}

function nodeMatchesSearch(
  node: RenderNode,
  term: string,
  files: ReturnType<typeof buildKnowledgeMapModel>["files"],
): boolean {
  const query = normalize(term);
  if (!query) {
    return true;
  }
  if (normalize(node.label).includes(query)) {
    return true;
  }
  return node.fileIds.some((fileId) => normalize(files[fileId]?.filename ?? "").includes(query));
}

function normalize(value: string) {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase()
    .trim();
}

function shorten(value: string, maximum: number) {
  return value.length <= maximum ? value : `${value.slice(0, maximum - 1)}…`;
}

function kindLabel(kind: RenderKind): string {
  const labels: Record<RenderKind, string> = {
    root: "Ordinateur",
    context: "Contexte",
    organization: "Entreprise / organisation",
    person: "Personne",
    project: "Projet",
    document_type: "Type de document",
    year: "Année",
    file: "Fichier",
  };
  return labels[kind];
}

function kindIcon(kind: RenderKind): string {
  const icons: Record<RenderKind, string> = {
    root: "⌂",
    context: "◫",
    organization: "▦",
    person: "●",
    project: "◆",
    document_type: "▤",
    year: "◷",
    file: "▧",
  };
  return icons[kind];
}

function coverageLabel(state: string): string {
  const labels: Record<string, string> = {
    accessible: "Accessible",
    authorization_required: "Autorisation requise",
    missing: "Absent",
    unsupported: "Protégé / non pris en charge",
    locked: "Verrouillé",
    permission_denied: "Accès refusé",
    temporarily_unavailable: "Temporairement indisponible",
    unexpected_error: "Erreur locale",
  };
  return labels[state.toLowerCase()] ?? state.replace(/_/g, " ");
}

function coverageTone(state: string): "ok" | "attention" | "muted" {
  const normalized = state.toLowerCase();
  if (normalized === "accessible") {
    return "ok";
  }
  if (normalized === "missing" || normalized === "unsupported") {
    return "muted";
  }
  return "attention";
}
