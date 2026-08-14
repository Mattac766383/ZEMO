import { useEffect, useMemo, useState } from "react";
import {
  approveExecution,
  cancelExecution,
  getErrorMessage,
  getExecutionStatus,
  getSystemStatus,
  listExecutionHistory,
  pauseExecution,
  prepareExecution,
  recoverExecution,
  rollbackExecution,
  selectAndRegisterRoot,
  setOrganizationProposalStatus,
  startExecution,
  subscribeExecutionProgress,
} from "./api";
import type {
  ExecutionDetail,
  ExecutionProgress,
  ExecutionSession,
  OrganizationProposal,
  RecoveryAssessment,
  RecoveryItem,
  SystemStatus,
} from "./types";

interface ExecutionPanelProps {
  workspaceId: string;
  proposal: OrganizationProposal;
  onReview?: () => void;
  onViewFiles?: () => void;
  onProposalUpdated?: (proposal: OrganizationProposal) => void;
}

type ExecutionBusy =
  | "prepare"
  | "apply"
  | "pause"
  | "cancel"
  | "rollback"
  | "recover"
  | "approve-proposal";

const ACTIVE_RECOVERY_STATES = new Set([
  "RECOVERY_REQUIRED",
  "RECOVERY_AVAILABLE",
  "RECOVERY_AMBIGUOUS",
]);

export function ExecutionPanel({
  workspaceId,
  proposal,
  onReview,
  onViewFiles,
  onProposalUpdated,
}: ExecutionPanelProps) {
  const [system, setSystem] = useState<SystemStatus | null>(null);
  const [execution, setExecution] = useState<ExecutionDetail | null>(null);
  const [history, setHistory] = useState<ExecutionSession[]>([]);
  const [progress, setProgress] = useState<ExecutionProgress | null>(null);
  const [recovery, setRecovery] = useState<RecoveryAssessment | null>(null);
  const [busy, setBusy] = useState<ExecutionBusy | null>(null);
  const [confirmationPhrase, setConfirmationPhrase] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [undoConfirmOpen, setUndoConfirmOpen] = useState(false);
  const [technicalOpen, setTechnicalOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let unsubscribe: (() => void) | undefined;
    void Promise.all([getSystemStatus(), listExecutionHistory(workspaceId)])
      .then(([status, sessions]) => {
        if (!active) {
          return;
        }
        setSystem(status);
        setHistory(sessions);
        const unfinished = sessions.find(
          (session) =>
            ACTIVE_RECOVERY_STATES.has(session.status) ||
            ACTIVE_RECOVERY_STATES.has(session.recoveryState) ||
            ["AWAITING_CONFIRMATION", "APPROVED", "PAUSED"].includes(session.status),
        );
        if (unfinished) {
          void getExecutionStatus(unfinished.id).then((detail) => {
            if (active) {
              setExecution(detail);
              if (detail.session.status === "AWAITING_CONFIRMATION") {
                setConfirmOpen(true);
              }
            }
          });
        }
      })
      .catch((reason) => {
        if (active) {
          setError(getErrorMessage(reason));
        }
      });
    void subscribeExecutionProgress((next) => {
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
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [workspaceId]);

  const recoverySession = useMemo(
    () =>
      history.find(
        (session) =>
          ACTIVE_RECOVERY_STATES.has(session.status) ||
          ACTIVE_RECOVERY_STATES.has(session.recoveryState),
      ) ?? null,
    [history],
  );

  useEffect(() => {
    setRecovery(null);
  }, [recoverySession?.id]);

  const filesToMove =
    proposal.summary.proposedMoves + proposal.summary.proposedRenames;
  const needsReview = proposal.summary.needsReview;
  const session = execution?.session;
  const awaitingConfirmation = session?.status === "AWAITING_CONFIRMATION";
  const consentInvalidated = session?.consentState === "INVALIDATED";
  const running = busy === "apply" || session?.status === "RUNNING";
  const completed = session
    ? [
        "COMPLETED",
        "PARTIAL",
        "FAILED",
        "CANCELLED",
        "ROLLED_BACK",
        "ROLLBACK_PARTIAL",
      ].includes(session.status)
    : false;
  const phraseValid =
    !session?.confirmationPhraseRequired || confirmationPhrase.trim() === "ORGANIZE";
  const applyReady =
    Boolean(system?.applyEnabled) &&
    !system?.journalLocked &&
    !recoverySession &&
    filesToMove > 0 &&
    proposal.status === "APPROVED_FOR_FUTURE_APPLY";

  async function refreshHistory(detail?: ExecutionDetail) {
    if (detail) {
      setExecution(detail);
    }
    setHistory(await listExecutionHistory(workspaceId));
    setSystem(await getSystemStatus());
  }

  async function requestApply() {
    setBusy("approve-proposal");
    setError(null);
    setProgress(null);
    try {
      let current = proposal;
      if (current.status !== "APPROVED_FOR_FUTURE_APPLY") {
        current = await setOrganizationProposalStatus(
          current.id,
          "approved_for_future_apply",
        );
        onProposalUpdated?.(current);
      }
      setBusy("prepare");
      const detail = await prepareExecution(current.id, current.revision);
      setExecution(detail);
      setConfirmationPhrase("");
      setConfirmOpen(true);
      await refreshHistory(detail);
    } catch (reason) {
      setError(getErrorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function apply() {
    if (!execution) {
      return;
    }
    setBusy("apply");
    setError(null);
    try {
      const approved = await approveExecution(
        execution.session.id,
        confirmationPhrase || undefined,
      );
      setExecution(approved);
      const completedDetail = await startExecution(approved.session.id);
      setConfirmOpen(false);
      await refreshHistory(completedDetail);
    } catch (reason) {
      setError(getErrorMessage(reason));
      try {
        setExecution(await getExecutionStatus(execution.session.id));
      } catch {
        // The original fail-closed error remains the useful message.
      }
      await refreshHistory();
    } finally {
      setBusy(null);
    }
  }

  async function requestPause() {
    if (!execution) {
      return;
    }
    setBusy("pause");
    try {
      await pauseExecution(execution.session.id);
    } catch (reason) {
      setError(getErrorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function requestCancel() {
    if (!execution) {
      return;
    }
    setBusy("cancel");
    try {
      await cancelExecution(execution.session.id);
      setConfirmOpen(false);
      await refreshHistory(await getExecutionStatus(execution.session.id));
    } catch (reason) {
      setError(getErrorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function rollback(executionId?: string) {
    const targetExecutionId = executionId ?? execution?.session.id;
    if (!targetExecutionId) {
      return;
    }
    setBusy("rollback");
    setError(null);
    setProgress(null);
    try {
      await refreshHistory(await rollbackExecution(targetExecutionId));
    } catch (reason) {
      setError(getErrorMessage(reason));
      await refreshHistory();
    } finally {
      setBusy(null);
    }
  }

  async function recover(sessionToRecover: ExecutionSession) {
    setBusy("recover");
    setError(null);
    try {
      const assessment = await recoverExecution(sessionToRecover.id);
      setRecovery(assessment);
      await refreshHistory(await getExecutionStatus(sessionToRecover.id));
    } catch (reason) {
      setError(getErrorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="execution-panel" aria-labelledby="execution-title">
      <header>
        <div>
          <span className="eyebrow">Organisation</span>
          <h3 id="execution-title">Organisation prête</h3>
          <p>
            {filesToMove.toLocaleString()} fichiers seront déplacés
            {" · 0 supprimé · 0 écrasé"}
            {needsReview > 0
              ? ` · ${needsReview.toLocaleString()} nécessitent encore votre avis`
              : ""}
          </p>
          <p className="apply-gate-note">
            Vous pourrez annuler les changements depuis l’historique.
          </p>
        </div>
        <span className={system?.applyEnabled ? "gate-ready" : "gate-locked"}>
          {system?.applyEnabled ? "Prête" : "Application indisponible"}
        </span>
      </header>

      {recoverySession ? (
        <div className="recovery-card" role="alert">
          <div>
            <strong>Une organisation a été interrompue.</strong>
            <span>Nous avons retrouvé l’état de vos fichiers.</span>
          </div>
          <div className="execution-confirm-actions">
            {!recovery ? (
              <button
                type="button"
                disabled={busy !== null || system?.journalLocked}
                onClick={() => void recover(recoverySession)}
              >
                {busy === "recover" ? "Examen…" : "Examiner"}
              </button>
            ) : null}
            {recovery &&
            recovery.applied === 0 &&
            recovery.ambiguous === 0 ? (
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => {
                  setRecovery(null);
                  setExecution(null);
                }}
              >
                Continuer
              </button>
            ) : null}
            {recovery?.rollbackAvailable || recoverySession.rollbackAvailable ? (
              <button
                type="button"
                disabled={busy !== null || system?.journalLocked}
                onClick={() => setUndoConfirmOpen(true)}
              >
                Annuler les changements
              </button>
            ) : null}
          </div>
        </div>
      ) : null}

      {system?.journalLocked ? (
        <div className="journal-diagnostic" role="alert">
          <strong>Une opération précédente nécessite votre attention.</strong>
          <p>
            Les modifications de fichiers restent bloquées. Aucune réparation
            automatique n’a été tentée.
          </p>
        </div>
      ) : null}

      {!system?.applyEnabled ? (
        <p className="execution-gate-reason" role="status">
          {system?.applyGateReason ??
            "Cette version propose une organisation à examiner ; le déplacement réel des fichiers n’est pas disponible ici."}
        </p>
      ) : null}
      {error ? (
        <div className="error-banner" role="alert">
          <p>{error}</p>
          {/besoin d’accéder à ce dossier|n’autorise plus l’accès|macOS n’autorise plus/i.test(
            error,
          ) ? (
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => {
                void selectAndRegisterRoot(workspaceId)
                  .then(() => setError(null))
                  .catch((reason) => setError(getErrorMessage(reason)));
              }}
            >
              {/n’autorise plus l’accès|macOS n’autorise plus/i.test(error)
                ? "Réautoriser"
                : "Autoriser l’accès"}
            </button>
          ) : null}
        </div>
      ) : null}

      {!execution && !recoverySession ? (
        <div className="execution-ready">
          <div>
            <strong>Organisation prête</strong>
            <span>
              {filesToMove.toLocaleString()} fichiers seront déplacés
              {" · 0 supprimé · 0 écrasé"}
              {needsReview > 0
                ? ` · ${needsReview.toLocaleString()} nécessitent encore votre avis`
                : ""}
            </span>
            <span>
              Vos fichiers seront déplacés selon l’organisation affichée.
              Aucun fichier ne sera supprimé ni écrasé.
            </span>
          </div>
          <div className="execution-confirm-actions">
            {needsReview > 0 && onReview ? (
              <button type="button" disabled={busy !== null} onClick={onReview}>
                Examiner
              </button>
            ) : null}
            <button
              type="button"
              className="primary-action"
              disabled={
                busy !== null ||
                !system?.applyEnabled ||
                filesToMove === 0 ||
                Boolean(system?.journalLocked)
              }
              onClick={() => void requestApply()}
            >
              {busy === "prepare" || busy === "approve-proposal"
                ? "Vérification…"
                : "Appliquer l’organisation"}
            </button>
          </div>
        </div>
      ) : null}

      {confirmOpen && awaitingConfirmation && session ? (
        <div className="execution-confirmation" role="dialog" aria-modal="true">
          <span className="eyebrow">Confirmation</span>
          <h3>Appliquer cette organisation ?</h3>
          <div className="execution-summary" aria-label="Résumé de l’application">
            <Metric
              label="Fichiers qui seront déplacés"
              value={session.summary.filesToMove + session.summary.filesToRename}
            />
            <Metric label="Fichiers qui seront supprimés" value={0} />
            <Metric label="Fichiers qui seront écrasés" value={0} />
            <Metric
              label="À vérifier"
              value={session.summary.needsReview}
              danger
            />
          </div>
          <p className="execution-warning">
            {consentInvalidated ? (
              <strong>
                Cette confirmation n’est plus valable. Annulez-la et préparez une
                nouvelle organisation.
              </strong>
            ) : (
              <>
                Vos fichiers seront déplacés selon l’organisation affichée.
                Aucun fichier ne sera supprimé ni écrasé.
              </>
            )}
          </p>
          {session.confirmationPhraseRequired ? (
            <label>
              Ce lot est très important. Pour éviter une application
              accidentelle, saisissez exactement <strong>ORGANIZE</strong>
              <input
                type="text"
                value={confirmationPhrase}
                autoComplete="off"
                onChange={(event) => setConfirmationPhrase(event.currentTarget.value)}
              />
            </label>
          ) : null}
          <div className="execution-confirm-actions">
            <button type="button" disabled={busy !== null} onClick={() => void requestCancel()}>
              Annuler
            </button>
            <button
              type="button"
              className="danger-action"
              disabled={busy !== null || !phraseValid || consentInvalidated}
              onClick={() => void apply()}
            >
              {busy === "apply" ? "Application…" : "Appliquer"}
            </button>
          </div>
        </div>
      ) : null}

      {running && progress ? (
        <div className="proposal-progress execution-progress" aria-live="polite">
          <div>
            <strong>Organisation en cours</strong>
            <span>
              {progress.completed.toLocaleString()} / {progress.total.toLocaleString()}{" "}
              fichiers
            </span>
          </div>
          <progress max={Math.max(progress.total, 1)} value={progress.completed} />
          <div className="execution-progress-actions">
            <button type="button" disabled={busy === "pause"} onClick={() => void requestPause()}>
              Pause après ce fichier
            </button>
            <button type="button" disabled={busy === "cancel"} onClick={() => void requestCancel()}>
              Arrêter après ce fichier
            </button>
          </div>
        </div>
      ) : null}

      {completed && session ? (
        <div className="execution-complete" role="status">
          <span className="eyebrow">
            {session.status === "ROLLED_BACK"
              ? "Organisation annulée"
              : session.status === "PARTIAL" || session.summary.failed > 0
                ? "Organisation partielle"
                : "Organisation terminée"}
          </span>
          <h3>
            {session.summary.applied.toLocaleString()} fichiers organisés
            {session.summary.failed + session.summary.blocked > 0
              ? ` · ${(session.summary.failed + session.summary.blocked).toLocaleString()} nécessitent votre attention`
              : " · 0 erreur"}
          </h3>
          <div className="execution-complete-actions">
            {onViewFiles ? (
              <button type="button" onClick={onViewFiles}>
                Voir les fichiers
              </button>
            ) : null}
            <button
              type="button"
              onClick={() => setTechnicalOpen(true)}
            >
              Historique
            </button>
            <button
              type="button"
              disabled={
                busy !== null ||
                !session.rollbackAvailable ||
                session.recoveryState === "RECOVERY_AMBIGUOUS"
              }
              onClick={() => setUndoConfirmOpen(true)}
            >
              {busy === "rollback" ? "Annulation…" : "Annuler les changements"}
            </button>
          </div>
        </div>
      ) : null}

      {undoConfirmOpen ? (
        <div className="execution-confirmation" role="dialog" aria-modal="true">
          <span className="eyebrow">Confirmation</span>
          <h3>Annuler les changements ?</h3>
          <p className="execution-warning">
            Les fichiers seront replacés à leur emplacement précédent lorsque
            cela peut être fait sans écraser de modifications récentes.
          </p>
          <div className="execution-confirm-actions">
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => setUndoConfirmOpen(false)}
            >
              Retour
            </button>
            <button
              type="button"
              className="danger-action"
              disabled={busy !== null}
              onClick={() => {
                setUndoConfirmOpen(false);
                void rollback(recoverySession?.id);
              }}
            >
              {busy === "rollback" ? "Annulation…" : "Annuler les changements"}
            </button>
          </div>
        </div>
      ) : null}

      <details
        className="execution-history"
        open={technicalOpen}
        onToggle={(event) => setTechnicalOpen(event.currentTarget.open)}
      >
        <summary>Détails techniques</summary>
        {recovery ? (
          <div className="recovery-assessment">
            <div className="execution-summary" aria-label="Recovery assessment">
              <Metric label="Affected" value={recovery.affectedCount} />
              <Metric label="Verified applied" value={recovery.applied} />
              <Metric label="Verified not started" value={recovery.notStarted} />
              <Metric label="Unresolved / ambiguous" value={recovery.ambiguous} danger />
            </div>
            <p>{recovery.message}</p>
            {recovery.verifiedAppliedItems.length > 0 ? (
              <RecoveryItems
                title="Verified applied operations"
                items={recovery.verifiedAppliedItems}
              />
            ) : null}
            {recovery.verifiedNotStartedItems.length > 0 ? (
              <RecoveryItems
                title="Verified not-started operations"
                items={recovery.verifiedNotStartedItems}
              />
            ) : null}
            {recovery.ambiguousItems.length > 0 ? (
              <details className="recovery-items">
                <summary>Inspect ambiguous items</summary>
                <RecoveryItemList items={recovery.ambiguousItems} />
              </details>
            ) : null}
            <details className="recovery-items">
              <summary>Executor session and request facts</summary>
              <ul>
                {recovery.executorSessions.map((fact) => (
                  <li key={fact.sessionId}>
                    {fact.purpose.toLocaleLowerCase()} session {shortId(fact.sessionId)} ·
                    coordinator {fact.coordinatorPid} · child {fact.childPid ?? "unavailable"}
                  </li>
                ))}
                {recovery.executorRequests.map((fact) => (
                  <li key={fact.requestId}>
                    request {shortId(fact.requestId)} · {humanize(fact.direction)} ·{" "}
                    {humanize(fact.state)} · sequence {fact.requestSequence}
                  </li>
                ))}
              </ul>
            </details>
            {recovery.rollbackAvailable ? (
              <button
                type="button"
                disabled={busy !== null || system?.journalLocked}
                onClick={() => setUndoConfirmOpen(true)}
              >
                {busy === "rollback" ? "Annulation…" : "Annuler les changements"}
              </button>
            ) : null}
          </div>
        ) : null}
        {system?.journalLocked ? (
          <div className="journal-diagnostic">
            <strong>Authenticated execution journal locked</strong>
            <p>
              Supremacy is read-only. Recovery and rollback are unavailable unless journal
              integrity can be proven; no repair was attempted.
            </p>
            <ul>
              {system.journalDiagnostics.map((diagnostic) => (
                <li key={`${diagnostic.scope}-${diagnostic.code}-${diagnostic.executionId ?? ""}`}>
                  <strong>{humanize(diagnostic.scope)}</strong>: {diagnostic.message} (
                  {diagnostic.code})
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {history.length > 0 ? (
          <ul>
            {history.map((item) => (
              <li key={item.id}>
                <span>
                  <strong>{humanize(item.status)}</strong>
                  <small>{new Date(item.createdAt).toLocaleString()}</small>
                </span>
                <span>
                  {item.summary.affectedFiles.toLocaleString()} files · rollback{" "}
                  {item.rollbackAvailable ? "available" : "unavailable"}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p>Aucun historique pour le moment.</p>
        )}
      </details>
      <span hidden>{applyReady ? "ready" : "blocked"}</span>
    </section>
  );
}

function Metric({
  label,
  value,
  danger = false,
}: {
  label: string;
  value: number;
  danger?: boolean;
}) {
  return (
    <div className={danger && value > 0 ? "execution-metric danger" : "execution-metric"}>
      <span>{label}</span>
      <strong>{value.toLocaleString()}</strong>
    </div>
  );
}

function RecoveryItems({ title, items }: { title: string; items: RecoveryItem[] }) {
  return (
    <div className="recovery-items">
      <strong>{title}</strong>
      <RecoveryItemList items={items} />
    </div>
  );
}

function RecoveryItemList({ items }: { items: RecoveryItem[] }) {
  return (
    <ul>
      {items.map((item) => (
        <li key={`${item.operationId}-${item.direction}`}>
          <span>{item.item}</span>
          <small>
            {humanize(item.direction)}
            {item.reason ? ` · ${item.reason}` : ""}
          </small>
        </li>
      ))}
    </ul>
  );
}

function shortId(value: string) {
  return value.length > 12 ? `${value.slice(0, 12)}…` : value;
}

function humanize(value: string) {
  return value.toLocaleLowerCase().replace(/_/g, " ");
}
