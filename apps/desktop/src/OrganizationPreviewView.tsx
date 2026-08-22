import { useEffect, useMemo, useState } from "react";
import {
  cancelOrganizationProposal,
  generateOrganizationProposal,
  getLatestOrganizationProposal,
  refreshOrganizationProposalDrift,
  setOrganizationProposalOverride,
  setOrganizationProposalStatus,
  subscribeOrganizationProposalProgress,
} from "./api";
import { classifyUserError } from "./errors";
import type {
  OrganizationOperation,
  OrganizationProposal,
  OrganizationProposalProgress,
  VirtualProposalNode,
} from "./types";
import { ExecutionPanel } from "./ExecutionPanel";

interface OrganizationPreviewViewProps {
  workspaceId: string;
  rootId?: string;
}

type NodeMap = Map<string | null, VirtualProposalNode[]>;

const MAX_CHILDREN_PER_BRANCH = 60;
const MIN_ZOOM = 0.65;
const MAX_ZOOM = 1.55;
const ZOOM_STEP = 0.1;

export function OrganizationPreviewView({
  workspaceId,
  rootId,
}: OrganizationPreviewViewProps) {
  const [proposal, setProposal] = useState<OrganizationProposal | null>(null);
  const [progress, setProgress] = useState<OrganizationProposalProgress | null>(null);
  const [busy, setBusy] = useState<"build" | "cancel" | "drift" | "status" | "edit" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [zoom, setZoom] = useState(1);

  useEffect(() => {
    let active = true;
    let unsubscribe: (() => void) | undefined;

    void subscribeOrganizationProposalProgress((next) => {
      if (active) {
        setProgress(next);
      }
    }).then((stop) => {
      if (active) {
        unsubscribe = stop;
      } else {
        stop();
      }
    });

    void getLatestOrganizationProposal(workspaceId, rootId, {
      uiBound: true,
      operationLimit: 500,
    })
      .then((next) => {
        if (!active) {
          return;
        }
        installProposal(next, setProposal, setExpanded, setSelectedNodeId);
      })
      .catch(() => {
        // No proposal yet is a normal first-run state.
      });

    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [rootId, workspaceId]);

  const childrenByParent = useMemo(
    () => buildChildrenMap(proposal?.nodes ?? []),
    [proposal],
  );

  const nodeById = useMemo(
    () => new Map((proposal?.nodes ?? []).map((node) => [node.id, node])),
    [proposal],
  );

  const operationById = useMemo(
    () => new Map((proposal?.operations ?? []).map((operation) => [operation.id, operation])),
    [proposal],
  );

  const operationByFileId = useMemo(
    () => new Map((proposal?.operations ?? []).map((operation) => [operation.fileId, operation])),
    [proposal],
  );

  const selectedNode = selectedNodeId ? nodeById.get(selectedNodeId) ?? null : null;
  const selectedOperation = selectedNode
    ? operationForNode(selectedNode, operationById, operationByFileId)
    : null;

  const matchingNodeIds = useMemo(() => {
    if (!proposal || !query.trim()) {
      return null;
    }
    const normalized = query.trim().toLocaleLowerCase();
    const directMatches = new Set<string>();
    for (const node of proposal.nodes) {
      const operation = operationForNode(node, operationById, operationByFileId);
      const searchable = [
        node.name,
        node.virtualPath,
        operation?.sourceRelativePath,
        operation?.proposedRelativePath,
        operation?.documentType,
        operation?.customerName,
        operation?.supplierName,
        operation?.projectName,
      ]
        .filter((value): value is string => Boolean(value))
        .join(" ")
        .toLocaleLowerCase();
      if (searchable.includes(normalized)) {
        directMatches.add(node.id);
      }
    }
    return includeAncestors(directMatches, nodeById);
  }, [nodeById, operationByFileId, operationById, proposal, query]);

  const roots = useMemo(() => {
    if (!proposal) {
      return [];
    }
    const explicitRoots = proposal.nodes.filter(
      (node) => node.kind === "ROOT" || !node.parentId || !nodeById.has(node.parentId),
    );
    return explicitRoots.length > 0 ? explicitRoots : proposal.nodes.slice(0, 1);
  }, [nodeById, proposal]);

  async function build(recompute: boolean) {
    setBusy("build");
    setError(null);
    setProgress(null);
    try {
      const next = await generateOrganizationProposal(workspaceId, recompute, rootId);
      installProposal(next, setProposal, setExpanded, setSelectedNodeId);
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function cancel() {
    setBusy("cancel");
    setError(null);
    try {
      await cancelOrganizationProposal(workspaceId);
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function refreshDrift() {
    if (!proposal) {
      return;
    }
    setBusy("drift");
    setError(null);
    try {
      setProposal(await refreshOrganizationProposalDrift(proposal.id));
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function updateStatus(status: "reviewed" | "approved_for_future_apply") {
    if (!proposal) {
      return;
    }
    setBusy("status");
    setError(null);
    try {
      setProposal(await setOrganizationProposalStatus(proposal.id, status));
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function applyOverride(
    operation: OrganizationOperation,
    action: "destination_and_rename" | "keep_in_place" | "to_review" | "reject",
    destination?: string[],
    proposedName?: string,
  ) {
    if (!proposal) {
      return;
    }
    setBusy("edit");
    setError(null);
    try {
      const next = await setOrganizationProposalOverride(
        proposal.id,
        operation.fileId,
        action,
        destination,
        proposedName,
        "Decision from ZEMO mind map",
      );
      setProposal(next);
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  function toggleNode(nodeId: string) {
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

  function expandSearchMatches() {
    if (!matchingNodeIds) {
      return;
    }
    setExpanded((current) => {
      const next = new Set(current);
      matchingNodeIds.forEach((id) => next.add(id));
      return next;
    });
  }

  if (!proposal) {
    return (
      <section className="proposal-empty zemo-map-empty" aria-labelledby="organization-preview-title">
        <style>{mindMapCss}</style>
        <span className="eyebrow">ZEMO · Carte mentale</span>
        <h2 id="organization-preview-title">Comprendre votre ordinateur</h2>
        <p>
          ZEMO construit une carte locale de vos dossiers, fichiers, projets et
          catégories. Rien n’est déplacé pendant cette étape.
        </p>
        <div className="proposal-safety-banner" role="status">
          <strong>APERÇU UNIQUEMENT</strong>
          <span>0 fichier modifié tant que vous n’avez rien validé.</span>
        </div>
        {progress ? <BuildProgress progress={progress} /> : null}
        {error ? <p className="error-banner">{error}</p> : null}
        <div className="zemo-empty-actions">
          <button
            type="button"
            className="primary-action"
            disabled={busy !== null}
            onClick={() => void build(false)}
          >
            {busy === "build" ? "Construction de la carte…" : "Construire la carte mentale"}
          </button>
          {busy === "build" ? (
            <button type="button" onClick={() => void cancel()}>
              Annuler
            </button>
          ) : null}
        </div>
      </section>
    );
  }

  return (
    <section className="proposal-preview zemo-map-page" aria-labelledby="organization-preview-title">
      <style>{mindMapCss}</style>

      <header className="zemo-map-header">
        <div>
          <span className="eyebrow">ZEMO · Vue mentale</span>
          <h2 id="organization-preview-title">Carte mentale de vos fichiers</h2>
          <p>
            Explorez progressivement la structure comprise par ZEMO. Les branches
            restent repliées pour garder la carte lisible même avec beaucoup de fichiers.
          </p>
        </div>
        <div className="zemo-map-header-actions">
          <button type="button" disabled={busy !== null} onClick={() => void refreshDrift()}>
            Vérifier les changements
          </button>
          <button type="button" disabled={busy !== null} onClick={() => void build(true)}>
            Recalculer la carte
          </button>
        </div>
      </header>

      <div className="proposal-safety-banner" role="status">
        <strong>PROPOSÉ — PAS APPLIQUÉ</strong>
        <span>La carte est une représentation locale. Vos fichiers restent à leur place.</span>
      </div>

      {progress && busy === "build" ? <BuildProgress progress={progress} /> : null}
      {error ? <p className="error-banner">{error}</p> : null}

      <div className="zemo-map-metrics" aria-label="Résumé de l’organisation">
        <Metric label="Fichiers compris" value={proposal.summary.filesAnalyzed} />
        <Metric label="À ranger" value={proposal.summary.proposedMoves} />
        <Metric label="Renommages" value={proposal.summary.proposedRenames} />
        <Metric label="À vérifier" value={proposal.summary.needsReview} tone="review" />
        <Metric label="Conflits" value={proposal.summary.conflicts} tone="danger" />
        <Metric label="Confiance élevée" value={proposal.summary.highConfidence} tone="good" />
      </div>

      <div className="zemo-map-toolbar">
        <label className="zemo-map-search">
          <span>Rechercher dans la carte</span>
          <input
            type="search"
            value={query}
            placeholder="Client, projet, facture, dossier, fichier…"
            onChange={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                expandSearchMatches();
              }
            }}
          />
        </label>
        <div className="zemo-map-zoom" aria-label="Zoom de la carte">
          <button
            type="button"
            aria-label="Dézoomer"
            onClick={() => setZoom((value) => clampZoom(value - ZOOM_STEP))}
          >
            −
          </button>
          <button type="button" onClick={() => setZoom(1)}>
            {Math.round(zoom * 100)} %
          </button>
          <button
            type="button"
            aria-label="Zoomer"
            onClick={() => setZoom((value) => clampZoom(value + ZOOM_STEP))}
          >
            +
          </button>
        </div>
        <button
          type="button"
          onClick={() => {
            setExpanded(new Set(initialExpanded(proposal.nodes)));
            setZoom(1);
            setQuery("");
          }}
        >
          Recentrer
        </button>
        <button
          type="button"
          onClick={() => setExpanded(new Set(proposal.nodes.map((node) => node.id)))}
        >
          Tout développer
        </button>
        <button type="button" onClick={() => setExpanded(new Set(initialExpanded(proposal.nodes)))}>
          Replier
        </button>
      </div>

      <div className="zemo-map-layout">
        <div className="zemo-map-viewport" aria-label="Carte mentale interactive">
          <div
            className="zemo-map-canvas"
            style={{
              transform: `scale(${zoom})`,
              transformOrigin: "top left",
              width: `${100 / zoom}%`,
            }}
          >
            {roots.length === 0 ? (
              <p className="empty-state">Aucune branche à afficher.</p>
            ) : (
              <div className="zemo-map-roots">
                {roots.map((node) => (
                  <MindBranch
                    key={node.id}
                    node={node}
                    childrenByParent={childrenByParent}
                    expanded={expanded}
                    selectedNodeId={selectedNodeId}
                    matchingNodeIds={matchingNodeIds}
                    level={0}
                    onSelect={setSelectedNodeId}
                    onToggle={toggleNode}
                  />
                ))}
              </div>
            )}
          </div>
        </div>

        <aside className="zemo-map-inspector" aria-label="Informations de la branche sélectionnée">
          {selectedNode ? (
            <NodeInspector
              node={selectedNode}
              operation={selectedOperation}
              disabled={busy !== null}
              onOverride={applyOverride}
            />
          ) : (
            <div className="zemo-inspector-empty">
              <span className="eyebrow">Détails</span>
              <h3>Sélectionnez une branche</h3>
              <p>
                Cliquez sur un dossier, un projet ou un fichier pour afficher toutes
                les informations disponibles ici.
              </p>
            </div>
          )}
        </aside>
      </div>

      <section className="zemo-map-validation" aria-label="Validation de la proposition">
        <div>
          <strong>Carte prête à être examinée</strong>
          <span>
            {proposal.summary.proposedMoves.toLocaleString()} déplacements proposés · {" "}
            {proposal.summary.needsReview.toLocaleString()} à vérifier · 0 suppression implicite
          </span>
        </div>
        <button type="button" disabled={busy !== null} onClick={() => void updateStatus("reviewed")}>
          Marquer comme examinée
        </button>
        <button
          type="button"
          className="primary-action"
          disabled={busy !== null || proposal.summary.needsReview > 0 || proposal.summary.conflicts > 0}
          onClick={() => void updateStatus("approved_for_future_apply")}
        >
          Valider pour application
        </button>
      </section>

      <ExecutionPanel
        workspaceId={workspaceId}
        proposal={proposal}
        onReview={() => {
          const firstReview = proposal.nodes.find((node) => node.needsReviewCount > 0);
          if (firstReview) {
            setSelectedNodeId(firstReview.id);
            setExpanded((current) => new Set([...current, firstReview.id]));
          }
        }}
        onViewFiles={() => {
          const firstFile = proposal.nodes.find((node) => node.kind === "FILE");
          if (firstFile) {
            setSelectedNodeId(firstFile.id);
          }
        }}
        onProposalUpdated={setProposal}
      />
    </section>
  );
}

function MindBranch({
  node,
  childrenByParent,
  expanded,
  selectedNodeId,
  matchingNodeIds,
  level,
  onSelect,
  onToggle,
}: {
  node: VirtualProposalNode;
  childrenByParent: NodeMap;
  expanded: Set<string>;
  selectedNodeId: string | null;
  matchingNodeIds: Set<string> | null;
  level: number;
  onSelect: (id: string) => void;
  onToggle: (id: string) => void;
}) {
  if (matchingNodeIds && !matchingNodeIds.has(node.id)) {
    return null;
  }

  const children = childrenByParent.get(node.id) ?? [];
  const isExpanded = expanded.has(node.id);
  const selected = selectedNodeId === node.id;
  const visibleChildren = children
    .filter((child) => !matchingNodeIds || matchingNodeIds.has(child.id))
    .slice(0, MAX_CHILDREN_PER_BRANCH);
  const hiddenChildren = Math.max(0, children.length - visibleChildren.length);
  const hasChildren = children.length > 0;

  return (
    <div className={`zemo-branch zemo-branch--level-${Math.min(level, 4)}`}>
      <div className="zemo-branch-row">
        {level > 0 ? <span className="zemo-connector" aria-hidden="true" /> : null}
        <button
          type="button"
          className={[
            "zemo-node",
            `zemo-node--${node.kind.toLocaleLowerCase()}`,
            selected ? "zemo-node--selected" : "",
          ]
            .filter(Boolean)
            .join(" ")}
          onClick={() => {
            onSelect(node.id);
            if (hasChildren && !isExpanded) {
              onToggle(node.id);
            }
          }}
          onDoubleClick={() => {
            if (hasChildren) {
              onToggle(node.id);
            }
          }}
          aria-expanded={hasChildren ? isExpanded : undefined}
        >
          <span className="zemo-node-icon" aria-hidden="true">
            {nodeIcon(node.kind)}
          </span>
          <span className="zemo-node-copy">
            <strong>{node.name}</strong>
            <small>
              {node.kind === "FILE"
                ? "Fichier"
                : `${node.childCount.toLocaleString()} élément${node.childCount > 1 ? "s" : ""}`}
            </small>
          </span>
          {node.needsReviewCount > 0 ? (
            <span className="zemo-node-badge zemo-node-badge--review">
              {node.needsReviewCount}
            </span>
          ) : null}
          {node.conflictCount > 0 ? (
            <span className="zemo-node-badge zemo-node-badge--danger">
              {node.conflictCount}
            </span>
          ) : null}
          {hasChildren ? <span className="zemo-node-chevron">{isExpanded ? "−" : "+"}</span> : null}
        </button>
      </div>

      {hasChildren && isExpanded ? (
        <div className="zemo-branch-children">
          {visibleChildren.map((child) => (
            <MindBranch
              key={child.id}
              node={child}
              childrenByParent={childrenByParent}
              expanded={expanded}
              selectedNodeId={selectedNodeId}
              matchingNodeIds={matchingNodeIds}
              level={level + 1}
              onSelect={onSelect}
              onToggle={onToggle}
            />
          ))}
          {hiddenChildren > 0 ? (
            <div className="zemo-more-node">+ {hiddenChildren.toLocaleString()} éléments masqués</div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function NodeInspector({
  node,
  operation,
  disabled,
  onOverride,
}: {
  node: VirtualProposalNode;
  operation: OrganizationOperation | null;
  disabled: boolean;
  onOverride: (
    operation: OrganizationOperation,
    action: "destination_and_rename" | "keep_in_place" | "to_review" | "reject",
    destination?: string[],
    proposedName?: string,
  ) => Promise<void>;
}) {
  const [destination, setDestination] = useState(operation?.proposedDestination.join(" / ") ?? "");
  const [name, setName] = useState(operation?.proposedName ?? "");

  useEffect(() => {
    setDestination(operation?.proposedDestination.join(" / ") ?? "");
    setName(operation?.proposedName ?? "");
  }, [operation?.id, operation?.proposedDestination, operation?.proposedName]);

  return (
    <div className="zemo-inspector-content">
      <div className="zemo-inspector-title">
        <span className="zemo-node-icon" aria-hidden="true">{nodeIcon(node.kind)}</span>
        <div>
          <span className="eyebrow">{friendlyKind(node.kind)}</span>
          <h3>{node.name}</h3>
        </div>
      </div>

      <dl className="zemo-info-grid">
        <Info label="Chemin mental" value={node.virtualPath || "Racine"} mono />
        <Info label="Éléments enfants" value={node.childCount.toLocaleString()} />
        <Info label="À vérifier" value={node.needsReviewCount.toLocaleString()} />
        <Info label="Conflits" value={node.conflictCount.toLocaleString()} />
      </dl>

      {operation ? (
        <>
          <div className="zemo-inspector-section">
            <h4>Informations fichier</h4>
            <dl className="zemo-info-grid">
              <Info label="Nom actuel" value={operation.sourceName} />
              <Info label="Emplacement actuel" value={operation.sourceRelativePath} mono />
              <Info label="Taille" value={formatBytes(operation.sourceByteSize)} />
              <Info label="Modifié" value={formatTimestamp(operation.sourceModifiedAt)} />
              <Info label="Type compris" value={friendlyValue(operation.documentType)} />
              <Info label="Contexte" value={friendlyValue(operation.semanticContext)} />
              <Info label="Client" value={friendlyValue(operation.customerName)} />
              <Info label="Fournisseur" value={friendlyValue(operation.supplierName)} />
              <Info label="Projet" value={friendlyValue(operation.projectName)} />
              <Info
                label="Confiance"
                value={`${Math.round(operation.confidenceScore * 100)} % · ${friendlyValue(operation.confidenceLevel)}`}
              />
              <Info label="Action" value={friendlyValue(operation.operationKind)} />
              <Info label="Conflit" value={friendlyValue(operation.conflictState)} />
            </dl>
          </div>

          <div className="zemo-inspector-section">
            <h4>Organisation proposée</h4>
            <label className="zemo-editor-field">
              <span>Dossier</span>
              <input
                value={destination}
                disabled={disabled}
                onChange={(event) => setDestination(event.currentTarget.value)}
              />
            </label>
            <label className="zemo-editor-field">
              <span>Nom du fichier</span>
              <input value={name} disabled={disabled} onChange={(event) => setName(event.currentTarget.value)} />
            </label>
            <button
              type="button"
              disabled={disabled || !name.trim()}
              onClick={() =>
                void onOverride(
                  operation,
                  "destination_and_rename",
                  splitDestination(destination),
                  name.trim(),
                )
              }
            >
              Enregistrer la proposition
            </button>
          </div>

          {operation.reasons.length > 0 ? (
            <details className="zemo-why">
              <summary>Pourquoi ZEMO propose ça ?</summary>
              <ul>
                {operation.reasons.map((reason, index) => (
                  <li key={`${reason.code}-${index}`}>
                    <strong>{friendlyValue(reason.code)}</strong>
                    <span>{reason.explanation}</span>
                  </li>
                ))}
              </ul>
            </details>
          ) : null}

          <div className="zemo-decision-actions">
            <button
              type="button"
              disabled={disabled}
              onClick={() => void onOverride(operation, "keep_in_place")}
            >
              Garder à sa place
            </button>
            <button
              type="button"
              disabled={disabled}
              onClick={() => void onOverride(operation, "to_review")}
            >
              À vérifier
            </button>
            <button
              type="button"
              className="danger-outline"
              disabled={disabled}
              onClick={() => void onOverride(operation, "reject")}
            >
              Rejeter cette proposition
            </button>
          </div>
        </>
      ) : (
        <div className="zemo-inspector-section">
          <h4>Résumé</h4>
          <p>
            Cette branche regroupe les éléments que ZEMO considère comme liés dans
            l’organisation virtuelle. Développez-la pour voir les sous-dossiers et fichiers.
          </p>
        </div>
      )}
    </div>
  );
}

function BuildProgress({ progress }: { progress: OrganizationProposalProgress }) {
  const percent = progress.filesTotal > 0
    ? Math.min(100, Math.round((progress.filesEvaluated / progress.filesTotal) * 100))
    : 0;
  return (
    <div className="zemo-build-progress" aria-live="polite">
      <div>
        <strong>{friendlyValue(progress.phase)}</strong>
        <span>{progress.filesEvaluated.toLocaleString()} / {progress.filesTotal.toLocaleString()}</span>
      </div>
      <div className="zemo-progress-track" aria-hidden="true">
        <span style={{ width: `${percent}%` }} />
      </div>
    </div>
  );
}

function Metric({
  label,
  value,
  tone = "default",
}: {
  label: string;
  value: number;
  tone?: "default" | "review" | "danger" | "good";
}) {
  return (
    <div className={`zemo-map-metric zemo-map-metric--${tone}`}>
      <span>{label}</span>
      <strong>{value.toLocaleString()}</strong>
    </div>
  );
}

function Info({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd className={mono ? "zemo-mono" : undefined}>{value}</dd>
    </div>
  );
}

function buildChildrenMap(nodes: VirtualProposalNode[]): NodeMap {
  const result: NodeMap = new Map();
  for (const node of nodes) {
    const key = node.parentId ?? null;
    const bucket = result.get(key) ?? [];
    bucket.push(node);
    result.set(key, bucket);
  }
  for (const bucket of result.values()) {
    bucket.sort((left, right) => {
      const rank = kindRank(left.kind) - kindRank(right.kind);
      return rank !== 0 ? rank : left.name.localeCompare(right.name, "fr", { sensitivity: "base" });
    });
  }
  return result;
}

function operationForNode(
  node: VirtualProposalNode,
  operationById: Map<string, OrganizationOperation>,
  operationByFileId: Map<string, OrganizationOperation>,
): OrganizationOperation | null {
  if (node.operationId) {
    const byOperation = operationById.get(node.operationId);
    if (byOperation) {
      return byOperation;
    }
  }
  return operationByFileId.get(node.id) ?? null;
}

function includeAncestors(matches: Set<string>, nodeById: Map<string, VirtualProposalNode>): Set<string> {
  const included = new Set(matches);
  for (const id of matches) {
    let node = nodeById.get(id);
    const visited = new Set<string>();
    while (node?.parentId && !visited.has(node.parentId)) {
      visited.add(node.parentId);
      included.add(node.parentId);
      node = nodeById.get(node.parentId);
    }
  }
  return included;
}

function initialExpanded(nodes: VirtualProposalNode[]): string[] {
  const roots = nodes.filter((node) => node.kind === "ROOT" || !node.parentId);
  const rootIds = new Set(roots.map((node) => node.id));
  return [
    ...rootIds,
    ...nodes.filter((node) => node.parentId && rootIds.has(node.parentId)).map((node) => node.id),
  ];
}

function installProposal(
  proposal: OrganizationProposal,
  setProposal: (proposal: OrganizationProposal) => void,
  setExpanded: (value: Set<string>) => void,
  setSelectedNodeId: (value: string | null) => void,
) {
  setProposal(proposal);
  setExpanded(new Set(initialExpanded(proposal.nodes)));
  setSelectedNodeId(
    proposal.nodes.find((node) => node.kind === "ROOT")?.id ?? proposal.nodes[0]?.id ?? null,
  );
}

function splitDestination(value: string): string[] {
  return value
    .split(/[\\/]+/)
    .map((segment) => segment.trim())
    .filter(Boolean);
}

function clampZoom(value: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, Math.round(value * 100) / 100));
}

function kindRank(kind: VirtualProposalNode["kind"]): number {
  switch (kind) {
    case "ROOT":
      return 0;
    case "FOLDER":
      return 1;
    case "FILE":
      return 2;
    default:
      return 3;
  }
}

function nodeIcon(kind: VirtualProposalNode["kind"]): string {
  switch (kind) {
    case "ROOT":
      return "◎";
    case "FOLDER":
      return "◇";
    case "FILE":
      return "▤";
    default:
      return "•";
  }
}

function friendlyKind(kind: string): string {
  switch (kind.toLocaleUpperCase()) {
    case "ROOT":
      return "Racine";
    case "FOLDER":
      return "Dossier / groupe";
    case "FILE":
      return "Fichier";
    default:
      return friendlyValue(kind);
  }
}

function friendlyValue(value?: string | null): string {
  if (!value || value.trim() === "") {
    return "Non disponible";
  }
  return value
    .replace(/_/g, " ")
    .toLocaleLowerCase()
    .replace(/(^|\s)\S/g, (character) => character.toLocaleUpperCase());
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatTimestamp(value?: string | null): string {
  if (!value) {
    return "Non disponible";
  }
  try {
    const milliseconds = Number(BigInt(value) / 1_000_000n);
    const date = new Date(milliseconds);
    if (!Number.isNaN(date.getTime())) {
      return date.toLocaleString();
    }
  } catch {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) {
      return date.toLocaleString();
    }
  }
  return "Non disponible";
}

const mindMapCss = `
.zemo-map-page,.zemo-map-empty{--zemo-border:rgba(148,163,184,.18);--zemo-panel:rgba(15,23,42,.56);--zemo-panel-strong:rgba(15,23,42,.82);--zemo-muted:#94a3b8;--zemo-text:#e5edf8;--zemo-accent:#7dd3fc;--zemo-accent-strong:#38bdf8;--zemo-good:#86efac;--zemo-review:#fde68a;--zemo-danger:#fca5a5;color:var(--zemo-text)}
.zemo-map-header{display:flex;justify-content:space-between;gap:24px;align-items:flex-start;margin-bottom:18px}.zemo-map-header h2{font-size:clamp(24px,3vw,38px);margin:4px 0 8px}.zemo-map-header p{max-width:780px;color:var(--zemo-muted);margin:0;line-height:1.55}.zemo-map-header-actions,.zemo-empty-actions{display:flex;gap:10px;flex-wrap:wrap}.zemo-map-metrics{display:grid;grid-template-columns:repeat(6,minmax(0,1fr));gap:10px;margin:16px 0}.zemo-map-metric{padding:14px;border:1px solid var(--zemo-border);border-radius:16px;background:var(--zemo-panel)}.zemo-map-metric span{display:block;color:var(--zemo-muted);font-size:12px;margin-bottom:6px}.zemo-map-metric strong{font-size:22px}.zemo-map-metric--review strong{color:var(--zemo-review)}.zemo-map-metric--danger strong{color:var(--zemo-danger)}.zemo-map-metric--good strong{color:var(--zemo-good)}
.zemo-map-toolbar{display:flex;gap:10px;align-items:end;flex-wrap:wrap;padding:12px;border:1px solid var(--zemo-border);border-radius:18px;background:var(--zemo-panel);margin-bottom:12px}.zemo-map-search{flex:1;min-width:260px}.zemo-map-search span,.zemo-editor-field span{display:block;color:var(--zemo-muted);font-size:12px;margin:0 0 6px}.zemo-map-search input,.zemo-editor-field input{width:100%;box-sizing:border-box}.zemo-map-zoom{display:flex;gap:6px}.zemo-map-zoom button{min-width:42px}
.zemo-map-layout{display:grid;grid-template-columns:minmax(0,1fr) minmax(300px,390px);gap:12px;min-height:620px}.zemo-map-viewport{overflow:auto;border:1px solid var(--zemo-border);border-radius:22px;background:radial-gradient(circle at 20% 15%,rgba(56,189,248,.07),transparent 28%),linear-gradient(rgba(148,163,184,.035) 1px,transparent 1px),linear-gradient(90deg,rgba(148,163,184,.035) 1px,transparent 1px),rgba(2,6,23,.48);background-size:auto,24px 24px,24px 24px,auto;padding:30px;min-height:620px}.zemo-map-canvas{min-width:760px;transition:transform .16s ease}.zemo-map-roots{display:flex;flex-direction:column;gap:18px}.zemo-branch{position:relative}.zemo-branch-row{display:flex;align-items:center;position:relative}.zemo-connector{width:30px;height:2px;background:linear-gradient(90deg,rgba(125,211,252,.2),rgba(125,211,252,.72));margin-right:6px;flex:0 0 30px}.zemo-branch-children{position:relative;margin-left:34px;padding-left:24px;border-left:1px solid rgba(125,211,252,.25);display:flex;flex-direction:column;gap:9px;margin-top:9px}.zemo-node{display:flex;align-items:center;gap:10px;text-align:left;min-width:210px;max-width:420px;padding:10px 12px;border-radius:15px;border:1px solid var(--zemo-border);background:rgba(15,23,42,.88);box-shadow:0 12px 28px rgba(0,0,0,.16);transition:transform .12s ease,border-color .12s ease,background .12s ease}.zemo-node:hover{transform:translateY(-1px);border-color:rgba(125,211,252,.48)}.zemo-node--root{padding:14px 16px;border-color:rgba(56,189,248,.45);background:linear-gradient(135deg,rgba(14,116,144,.32),rgba(15,23,42,.92))}.zemo-node--selected{outline:2px solid rgba(56,189,248,.72);outline-offset:2px}.zemo-node-icon{display:grid;place-items:center;width:30px;height:30px;flex:0 0 30px;border-radius:10px;background:rgba(56,189,248,.1);color:var(--zemo-accent);font-size:18px}.zemo-node-copy{min-width:0;flex:1}.zemo-node-copy strong{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.zemo-node-copy small{display:block;color:var(--zemo-muted);font-size:11px;margin-top:2px}.zemo-node-badge{font-size:10px;padding:3px 6px;border-radius:999px}.zemo-node-badge--review{background:rgba(250,204,21,.12);color:var(--zemo-review)}.zemo-node-badge--danger{background:rgba(248,113,113,.12);color:var(--zemo-danger)}.zemo-node-chevron{color:var(--zemo-muted);font-size:18px}.zemo-more-node{color:var(--zemo-muted);font-size:12px;padding:8px 12px}
.zemo-map-inspector{border:1px solid var(--zemo-border);border-radius:22px;background:var(--zemo-panel-strong);padding:18px;overflow:auto;max-height:760px;position:sticky;top:12px}.zemo-inspector-title{display:flex;align-items:center;gap:12px;padding-bottom:14px;border-bottom:1px solid var(--zemo-border)}.zemo-inspector-title h3{margin:3px 0 0;font-size:20px;overflow-wrap:anywhere}.zemo-info-grid{display:grid;grid-template-columns:1fr;gap:0;margin:12px 0}.zemo-info-grid>div{display:grid;grid-template-columns:120px minmax(0,1fr);gap:10px;padding:9px 0;border-bottom:1px solid rgba(148,163,184,.09)}.zemo-info-grid dt{color:var(--zemo-muted);font-size:12px}.zemo-info-grid dd{margin:0;overflow-wrap:anywhere}.zemo-mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}.zemo-inspector-section{margin-top:18px;padding-top:14px;border-top:1px solid var(--zemo-border)}.zemo-inspector-section h4{margin:0 0 10px}.zemo-editor-field{display:block;margin-bottom:10px}.zemo-why{margin:16px 0}.zemo-why ul{padding-left:18px}.zemo-why li{margin:8px 0}.zemo-why li span{display:block;color:var(--zemo-muted);font-size:12px;margin-top:2px}.zemo-decision-actions{display:flex;gap:8px;flex-wrap:wrap;margin-top:16px}.zemo-inspector-empty{padding:24px 8px}.zemo-inspector-empty p{color:var(--zemo-muted);line-height:1.55}
.zemo-map-validation{display:flex;align-items:center;gap:10px;margin-top:14px;padding:14px 16px;border:1px solid var(--zemo-border);border-radius:18px;background:var(--zemo-panel)}.zemo-map-validation>div{display:flex;flex-direction:column;gap:3px;margin-right:auto}.zemo-map-validation span{font-size:12px;color:var(--zemo-muted)}.zemo-build-progress{padding:12px 14px;border:1px solid var(--zemo-border);border-radius:15px;background:var(--zemo-panel);margin:12px 0}.zemo-build-progress>div:first-child{display:flex;justify-content:space-between;gap:12px;margin-bottom:8px}.zemo-build-progress span{color:var(--zemo-muted)}.zemo-progress-track{height:6px;border-radius:999px;background:rgba(148,163,184,.12);overflow:hidden}.zemo-progress-track span{display:block;height:100%;background:linear-gradient(90deg,#0ea5e9,#7dd3fc);border-radius:inherit;transition:width .2s ease}
@media (max-width:1100px){.zemo-map-metrics{grid-template-columns:repeat(3,minmax(0,1fr))}.zemo-map-layout{grid-template-columns:1fr}.zemo-map-inspector{position:static;max-height:none}.zemo-map-header{flex-direction:column}.zemo-map-validation{align-items:stretch;flex-direction:column}.zemo-map-validation>div{margin-right:0}}
@media (max-width:700px){.zemo-map-metrics{grid-template-columns:repeat(2,minmax(0,1fr))}.zemo-map-viewport{padding:18px}.zemo-map-canvas{min-width:620px}}
`;
