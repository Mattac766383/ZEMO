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

type ProposalFilter =
  | "all"
  | "review"
  | "conflicts"
  | "high"
  | "unchanged"
  | "renames";

interface OrganizationPreviewViewProps {
  workspaceId: string;
  rootId?: string;
}

const MAX_VISIBLE_OPERATIONS = 250;

export function OrganizationPreviewView({
  workspaceId,
  rootId,
}: OrganizationPreviewViewProps) {
  const [proposal, setProposal] = useState<OrganizationProposal | null>(null);
  const [progress, setProgress] = useState<OrganizationProposalProgress | null>(null);
  const [selectedOperationId, setSelectedOperationId] = useState<string | null>(null);
  const [selectedFolder, setSelectedFolder] = useState("");
  const [filter, setFilter] = useState<ProposalFilter>("all");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<
    "build" | "cancel" | "edit" | "status" | "drift" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  // Feature-local errors only — never escalate to a global catastrophic banner.

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
        if (active) {
          setProposal(next);
        }
      })
      .catch(() => {
        // A workspace without a proposal is the normal initial state.
      });
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [rootId, workspaceId]);

  const selectedOperation = useMemo(
    () =>
      proposal?.operations.find((operation) => operation.id === selectedOperationId) ??
      null,
    [proposal, selectedOperationId],
  );

  const filteredOperations = useMemo(() => {
    if (!proposal) {
      return [];
    }
    const normalizedQuery = query.trim().toLocaleLowerCase();
    return proposal.operations.filter((operation) => {
      if (
        selectedFolder &&
        !operation.proposedDestination
          .join("\\")
          .toLocaleLowerCase()
          .startsWith(selectedFolder.toLocaleLowerCase())
      ) {
        return false;
      }
      if (filter === "review" && !operation.needsReview) {
        return false;
      }
      if (filter === "conflicts" && operation.conflictState === "NONE") {
        return false;
      }
      if (
        filter === "high" &&
        !["VERY_HIGH", "HIGH"].includes(operation.confidenceLevel)
      ) {
        return false;
      }
      if (
        filter === "unchanged" &&
        !["KEEP_IN_PLACE", "NO_ACTION"].includes(operation.operationKind)
      ) {
        return false;
      }
      if (
        filter === "renames" &&
        operation.sourceName.toLocaleLowerCase() ===
          operation.proposedName.toLocaleLowerCase()
      ) {
        return false;
      }
      if (!normalizedQuery) {
        return true;
      }
      return [
        operation.sourceRelativePath,
        operation.proposedRelativePath,
        operation.customerName,
        operation.supplierName,
        operation.projectName,
        operation.documentType,
      ]
        .filter(Boolean)
        .some((value) => value!.toLocaleLowerCase().includes(normalizedQuery));
    });
  }, [filter, proposal, query, selectedFolder]);

  const folders = useMemo(
    () => proposal?.nodes?.filter((node) => node.kind !== "FILE") ?? [],
    [proposal],
  );

  async function build(recompute: boolean) {
    setBusy("build");
    setError(null);
    setProgress(null);
    try {
      // Backend returns a UI-bounded projection (folder nodes + capped operations).
      const next = await generateOrganizationProposal(
        workspaceId,
        recompute,
        rootId,
      );
      setProposal(next);
      setSelectedOperationId(next.operations[0]?.id ?? null);
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function cancel() {
    setBusy("cancel");
    try {
      await cancelOrganizationProposal(workspaceId);
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function updateStatus(
    status: "reviewed" | "approved_for_future_apply",
  ) {
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

  async function checkDrift() {
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

  async function saveOverride(
    operation: OrganizationOperation,
    destination: string[],
    proposedName: string,
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
        "destination_and_rename",
        destination,
        proposedName,
        "Virtual edit from organization preview",
      );
      setProposal(next);
      setSelectedOperationId(
        next.operations.find((item) => item.fileId === operation.fileId)?.id ?? null,
      );
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  async function decideOverride(
    operation: OrganizationOperation,
    action: "keep_in_place" | "to_review" | "reject",
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
        undefined,
        undefined,
        "User decision from organization preview",
      );
      setProposal(next);
      setSelectedOperationId(
        next.operations.find((item) => item.fileId === operation.fileId)?.id ?? null,
      );
    } catch (reason) {
      setError(classifyUserError(reason, "organization").message);
    } finally {
      setBusy(null);
    }
  }

  if (!proposal) {
    return (
      <section className="proposal-empty" aria-labelledby="organization-preview-title">
        <span className="eyebrow">Aperçu · Avant toute modification</span>
        <h2 id="organization-preview-title">Organisation proposée</h2>
        <p>
          Construisez une organisation virtuelle à partir du catalogue local,
          de la compréhension et des relations confirmées.
        </p>
        <div className="proposal-safety-banner" role="status">
          <strong>APERÇU UNIQUEMENT</strong>
          <span>Rien n’a encore été modifié sur votre ordinateur.</span>
        </div>
        {progress ? <BuildProgress progress={progress} /> : null}
        {error ? <p className="error-banner">{error}</p> : null}
        <button
          type="button"
          className="primary-action"
          disabled={busy !== null}
          onClick={() => void build(false)}
        >
          {busy === "build" ? "Préparation…" : "Préparer l’organisation"}
        </button>
          {busy === "build" ? (
          <button type="button" onClick={() => void cancel()}>
            Annuler en toute sécurité
          </button>
        ) : null}
      </section>
    );
  }

  return (
    <section className="proposal-preview" aria-labelledby="organization-preview-title">
      <header className="proposal-preview-header">
        <div>
          <span className="eyebrow">Organisation proposée</span>
          <h2 id="organization-preview-title">Organisation proposée</h2>
          <p>
            Comparez l’emplacement actuel et la destination proposée. Rien n’a
            encore été modifié sur votre ordinateur.
          </p>
        </div>
        <div className="proposal-actions">
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void checkDrift()}
          >
            Vérifier les changements sources
          </button>
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void build(true)}
          >
            Recalculer sans risque
          </button>
        </div>
      </header>

      <div className="proposal-safety-banner" role="status">
        <strong>PROPOSÉ — PAS ENCORE APPLIQUÉ</strong>
        <span>Rien n’a encore été modifié sur votre ordinateur.</span>
      </div>

      <div className="proposal-attention" aria-label="Priorité d’examen">
        <span className="attention-chip attention-chip--ready">
          Confiance élevée :{" "}
          {(
            proposal.summary.proposedMoves +
            proposal.summary.proposedRenames
          ).toLocaleString()}
        </span>
        <span className="attention-chip attention-chip--review">
          À vérifier : {proposal.summary.needsReview.toLocaleString()}
        </span>
        <span className="attention-chip attention-chip--blocked">
          Incertain : {proposal.summary.conflicts.toLocaleString()}
        </span>
      </div>

      {progress && busy === "build" ? <BuildProgress progress={progress} /> : null}
      {error ? <p className="error-banner">{error}</p> : null}

      <div className="proposal-summary" aria-label="Résumé de la proposition">
        <SummaryMetric label="Analysés" value={proposal.summary.filesAnalyzed} />
        <SummaryMetric label="Déplacements" value={proposal.summary.proposedMoves} />
        <SummaryMetric label="Renommages" value={proposal.summary.proposedRenames} />
        <SummaryMetric label="Inchangés" value={proposal.summary.unchanged} />
        <SummaryMetric label="À revoir" value={proposal.summary.needsReview} tone="review" />
        <SummaryMetric label="Conflits" value={proposal.summary.conflicts} tone="danger" />
      </div>

      <div className="proposal-toolbar" role="search">
        <label>
          Rechercher
          <input
            type="search"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder="Fichier, fournisseur, projet…"
          />
        </label>
        <label>
          Afficher
          <select
            value={filter}
            onChange={(event) => setFilter(event.currentTarget.value as ProposalFilter)}
          >
            <option value="all">Tous les éléments</option>
            <option value="review">À vérifier</option>
            <option value="conflicts">Incertain / conflit</option>
            <option value="high">Confiance élevée</option>
            <option value="unchanged">Inchangés</option>
            <option value="renames">Renommages proposés</option>
          </select>
        </label>
        {selectedFolder ? (
          <button type="button" onClick={() => setSelectedFolder("")}>
            Effacer le filtre de dossier
          </button>
        ) : null}
      </div>

      <div className="proposal-workspace">
        <aside className="virtual-tree" aria-label="Arborescence proposée">
          <div className="panel-heading">
            <strong>Proposition</strong>
            <span>{folders.length - 1} dossiers</span>
          </div>
          <FolderTree
            nodes={folders}
            selectedPath={selectedFolder}
            onSelect={setSelectedFolder}
          />
        </aside>

        <div className="proposal-file-list" aria-label="Éléments proposés">
          <div className="panel-heading">
            <strong>{selectedFolder || "Tous les emplacements"}</strong>
            <span>{filteredOperations.length} éléments</span>
          </div>
          {filteredOperations.length === 0 ? (
            <p className="empty-state">
              Aucun élément ne correspond à ces filtres.
            </p>
          ) : (
            <ul>
              {filteredOperations.slice(0, MAX_VISIBLE_OPERATIONS).map((operation) => (
                <li key={operation.id}>
                  <button
                    type="button"
                    className={
                      operation.id === selectedOperationId
                        ? "proposal-file selected"
                        : "proposal-file"
                    }
                    onClick={() => setSelectedOperationId(operation.id)}
                  >
                    <span>
                      <strong>{operation.proposedName}</strong>
                      <small>
                        {operation.proposedDestination.join(" / ") || "Emplacement actuel"}
                      </small>
                    </span>
                    <ConfidenceBadge operation={operation} />
                  </button>
                </li>
              ))}
            </ul>
          )}
          {filteredOperations.length > MAX_VISIBLE_OPERATIONS ? (
            <p className="result-limit">
              Affichage des {MAX_VISIBLE_OPERATIONS} premiers éléments. Affinez le
              dossier ou la recherche pour en voir davantage.
            </p>
          ) : null}
        </div>

        <aside className="proposal-detail" aria-label="Actuel et proposition">
          {selectedOperation ? (
            <OperationDetail
              key={selectedOperation.id}
              operation={selectedOperation}
              disabled={busy !== null}
              onSave={saveOverride}
              onDecision={decideOverride}
            />
          ) : (
            <p className="empty-state">
              Sélectionnez un fichier pour comparer l’emplacement actuel et la
              proposition.
            </p>
          )}
        </aside>
      </div>

      <footer className="proposal-review-footer">
        <div>
          <strong>Organisation prête</strong>
          <span>
            {(
              proposal.summary.proposedMoves + proposal.summary.proposedRenames
            ).toLocaleString()}{" "}
            fichiers seront déplacés · 0 supprimé · 0 écrasé
            {proposal.summary.needsReview > 0
              ? ` · ${proposal.summary.needsReview.toLocaleString()} nécessitent encore votre avis`
              : ""}
          </span>
          <span className="apply-gate-note">
            Vous pourrez annuler les changements depuis l’historique.
          </span>
        </div>
        {proposal.summary.needsReview > 0 ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => setFilter("review")}
          >
            Examiner
          </button>
        ) : null}
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void updateStatus("reviewed")}
        >
          Marquer l’examen comme terminé
        </button>
        <button
          type="button"
          disabled={busy !== null || proposal.status === "APPROVED_FOR_FUTURE_APPLY"}
          onClick={() => void updateStatus("approved_for_future_apply")}
        >
          Valider la proposition
        </button>
      </footer>
      <ExecutionPanel
        workspaceId={workspaceId}
        proposal={proposal}
        onReview={() => setFilter("review")}
        onViewFiles={() => setSelectedFolder("")}
        onProposalUpdated={setProposal}
      />
    </section>
  );
}

function BuildProgress({ progress }: { progress: OrganizationProposalProgress }) {
  const percentage =
    progress.filesTotal === 0
      ? 0
      : Math.round((progress.filesEvaluated / progress.filesTotal) * 100);
  return (
    <div className="proposal-progress" aria-live="polite">
      <div>
        <strong>{humanize(progress.phase)}</strong>
        <span>
          {progress.filesEvaluated.toLocaleString()} /{" "}
          {progress.filesTotal.toLocaleString()} fichiers
        </span>
      </div>
      <progress max={100} value={percentage}>
        {percentage}%
      </progress>
      <div className="progress-facts">
        <span>Confiance élevée : {progress.highConfidence.toLocaleString()}</span>
        <span>À vérifier : {progress.needsReview.toLocaleString()}</span>
        <span>Incertains : {progress.conflicts.toLocaleString()}</span>
      </div>
    </div>
  );
}

function FolderTree({
  nodes,
  selectedPath,
  onSelect,
}: {
  nodes: VirtualProposalNode[];
  selectedPath: string;
  onSelect: (path: string) => void;
}) {
  const root = nodes.find((node) => node.kind === "ROOT");
  if (!root) {
    return <p className="empty-state">Aucune proposition pour le moment.</p>;
  }
  const byParent = new Map<string, VirtualProposalNode[]>();
  for (const node of nodes) {
    if (node.kind !== "FOLDER" || !node.parentId) {
      continue;
    }
    const siblings = byParent.get(node.parentId) ?? [];
    siblings.push(node);
    byParent.set(node.parentId, siblings);
  }
  for (const siblings of byParent.values()) {
    siblings.sort((left, right) => left.name.localeCompare(right.name));
  }
  return (
    <div className="tree-root">
      <button
        type="button"
        className={selectedPath === "" ? "tree-node selected" : "tree-node"}
        onClick={() => onSelect("")}
      >
        <span>▾</span>
        <strong>Racine proposée</strong>
        <small>{root.childCount}</small>
      </button>
      <TreeChildren
        parentId={root.id}
        byParent={byParent}
        selectedPath={selectedPath}
        onSelect={onSelect}
        depth={0}
      />
    </div>
  );
}

function TreeChildren({
  parentId,
  byParent,
  selectedPath,
  onSelect,
  depth,
}: {
  parentId: string;
  byParent: Map<string, VirtualProposalNode[]>;
  selectedPath: string;
  onSelect: (path: string) => void;
  depth: number;
}) {
  const children = byParent.get(parentId) ?? [];
  return (
    <>
      {children.map((node) => (
        <div key={node.id}>
          <button
            type="button"
            className={
              selectedPath === node.virtualPath ? "tree-node selected" : "tree-node"
            }
            style={{ paddingInlineStart: `${16 + depth * 16}px` }}
            onClick={() => onSelect(node.virtualPath)}
            aria-label={`${node.name}, ${node.childCount} children, ${node.needsReviewCount} need review`}
          >
            <span>▸</span>
            <span>{node.name}</span>
            {node.conflictCount > 0 ? (
              <small className="tree-conflict">{node.conflictCount}</small>
            ) : (
              <small>{node.childCount}</small>
            )}
          </button>
          <TreeChildren
            parentId={node.id}
            byParent={byParent}
            selectedPath={selectedPath}
            onSelect={onSelect}
            depth={depth + 1}
          />
        </div>
      ))}
    </>
  );
}

function OperationDetail({
  operation,
  disabled,
  onSave,
  onDecision,
}: {
  operation: OrganizationOperation;
  disabled: boolean;
  onSave: (
    operation: OrganizationOperation,
    destination: string[],
    proposedName: string,
  ) => Promise<void>;
  onDecision: (
    operation: OrganizationOperation,
    action: "keep_in_place" | "to_review" | "reject",
  ) => Promise<void>;
}) {
  const [destination, setDestination] = useState(
    operation.proposedDestination.join("\\"),
  );
  const [name, setName] = useState(operation.proposedName);
  const destinationSegments = destination
    .split(/[\\/]/)
    .map((segment) => segment.trim())
    .filter(Boolean);
  const editValid =
    destinationSegments.length > 0 &&
    destinationSegments.length <= 8 &&
    name.trim().length > 0;
  return (
    <div>
      <div className="panel-heading">
        <strong>Pourquoi ici ?</strong>
        <ConfidenceBadge operation={operation} />
      </div>
      <dl className="path-comparison path-comparison--emphasis">
        <div className="path-comparison__current">
          <dt>Actuellement</dt>
          <dd>
            <code>{operation.sourceRelativePath}</code>
          </dd>
        </div>
        <div className="path-comparison__proposed">
          <dt>Proposition</dt>
          <dd>
            <code>{operation.proposedRelativePath}</code>
          </dd>
        </div>
      </dl>
      <div className="operation-signals">
        <span>{humanize(operation.semanticContext)}</span>
        <span>{humanize(operation.documentType)}</span>
        {operation.customerName ? <span>Client : {operation.customerName}</span> : null}
        {operation.projectName ? <span>Projet : {operation.projectName}</span> : null}
        {operation.supplierName ? (
          <span>Fournisseur : {operation.supplierName}</span>
        ) : null}
      </div>
      {operation.conflictState !== "NONE" ? (
        <p className="conflict-message">
          Conflit : {humanize(operation.conflictState)}
        </p>
      ) : null}
      <ul className="proposal-reasons">
        {operation.reasons.map((reason, index) => (
          <li key={`${reason.code}-${index}`}>
            <span aria-hidden="true">✓</span>
            <span>
              {reason.explanation}
              {reason.evidenceReferences.length > 0 ? (
                <small>{reason.evidenceReferences.join(" · ")}</small>
              ) : null}
            </span>
          </li>
        ))}
      </ul>
      <div className="virtual-decisions proposal-primary-actions">
        <button
          type="button"
          className="primary"
          disabled={disabled || !editValid}
          onClick={() => void onSave(operation, destinationSegments, name.trim())}
        >
          Accepter
        </button>
        <button
          type="button"
          disabled={disabled}
          onClick={() => void onDecision(operation, "to_review")}
        >
          À vérifier
        </button>
      </div>
      <p className="apply-gate-note">
        Accepter enregistre la proposition uniquement. Les fichiers ne sont
        déplacés que lorsque vous appliquez l’organisation.
      </p>
      <details className="virtual-edit">
        <summary>Modifier</summary>
        <fieldset>
          <legend>Modifier la proposition</legend>
          <label>
            Dossier proposé
            <input
              value={destination}
              onChange={(event) => setDestination(event.currentTarget.value)}
              aria-describedby="virtual-path-help"
            />
          </label>
          <small id="virtual-path-help">
            Séparez les dossiers par \. Les chemins absolus sont refusés.
          </small>
          <label>
            Nom proposé
            <input
              value={name}
              onChange={(event) => setName(event.currentTarget.value)}
            />
          </label>
          <button
            type="button"
            disabled={disabled || !editValid}
            onClick={() => void onSave(operation, destinationSegments, name.trim())}
          >
            Enregistrer la modification
          </button>
          <div className="virtual-decisions">
            <button
              type="button"
              disabled={disabled}
              onClick={() => void onDecision(operation, "keep_in_place")}
            >
              Garder l’emplacement actuel
            </button>
            <button
              type="button"
              disabled={disabled}
              onClick={() => void onDecision(operation, "reject")}
            >
              Refuser
            </button>
          </div>
        </fieldset>
      </details>
    </div>
  );
}

function attentionState(operation: OrganizationOperation): {
  label: string;
  kind: "ready" | "review" | "blocked";
} {
  const conflict = operation.conflictState.toLocaleUpperCase();
  if (
    conflict !== "NONE" &&
    conflict !== "AUTO_RESOLVED" &&
    conflict !== ""
  ) {
    return { label: "Incertain", kind: "blocked" };
  }
  if (operation.needsReview || operation.operationKind === "TO_REVIEW") {
    return { label: "À vérifier", kind: "review" };
  }
  if (["VERY_HIGH", "HIGH"].includes(operation.confidenceLevel)) {
    return { label: "Confiance élevée", kind: "ready" };
  }
  return { label: "À vérifier", kind: "review" };
}

function ConfidenceBadge({ operation }: { operation: OrganizationOperation }) {
  const attention = attentionState(operation);
  return (
    <span
      className={`confidence-badge confidence-${attention.kind}`}
      aria-label={attention.label}
      title="Niveau de confiance indicatif — ce n’est pas une probabilité calibrée."
    >
      {attention.label}
    </span>
  );
}

function SummaryMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "review" | "danger";
}) {
  return (
    <div className={tone ? `summary-metric ${tone}` : "summary-metric"}>
      <span>{label}</span>
      <strong>{value.toLocaleString()}</strong>
    </div>
  );
}

function humanize(value: string): string {
  return value
    .toLocaleLowerCase()
    .split("_")
    .map((part) => part.charAt(0).toLocaleUpperCase() + part.slice(1))
    .join(" ");
}
