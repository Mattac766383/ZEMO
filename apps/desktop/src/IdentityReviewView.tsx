import { useEffect, useState } from "react";
import {
  cancelIdentityResolution,
  decideIdentityCandidate,
  getErrorMessage,
  listIdentityReviewGroups,
  resolveIdentities,
  subscribeIdentityResolutionProgress,
} from "./api";
import type {
  IdentityCandidate,
  IdentityResolutionProgress,
  IdentityReviewPage,
} from "./types";

const PAGE_SIZE = 30;

interface IdentityReviewViewProps {
  workspaceId: string;
  onOpenIdentity: (identityId: string) => void;
}

export function IdentityReviewView({
  workspaceId,
  onOpenIdentity,
}: IdentityReviewViewProps) {
  const [status, setStatus] = useState<"needs_review" | "resolved" | "all">("needs_review");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<IdentityReviewPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [workingId, setWorkingId] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [resolution, setResolution] = useState<IdentityResolutionProgress | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void subscribeIdentityResolutionProgress((progress) => {
      if (active && progress.workspaceId === workspaceId) {
        setResolution(progress);
        if (progress.phase !== "RUNNING") {
          setRefresh((value) => value + 1);
        }
      }
    }).then((stop) => {
      if (active) {
        unlisten = stop;
      } else {
        stop();
      }
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [workspaceId]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    void listIdentityReviewGroups(workspaceId, status, PAGE_SIZE, offset)
      .then((next) => {
        if (active) {
          setPage(next);
        }
      })
      .catch((reason) => {
        if (active) {
          setError(getErrorMessage(reason));
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
  }, [offset, refresh, status, workspaceId]);

  async function decide(
    candidate: IdentityCandidate,
    action: "confirm" | "keep_separate",
  ) {
    setWorkingId(candidate.candidateId);
    setError(null);
    setNotice(null);
    try {
      await decideIdentityCandidate(
        candidate.candidateId,
        action,
        action === "confirm"
          ? "Correspondance confirmée depuis la revue d’identité"
          : "Identités gardées séparées depuis la revue",
      );
      setNotice(
        action === "confirm"
          ? "Les enregistrements sémantiques ont été reliés. Aucun fichier n’a été modifié."
          : "La séparation est mémorisée pour les prochaines analyses.",
      );
      setRefresh((value) => value + 1);
    } catch (reason) {
      setError(getErrorMessage(reason));
    } finally {
      setWorkingId(null);
    }
  }

  async function runResolution() {
    if (resolving) {
      return;
    }
    setResolving(true);
    setError(null);
    setNotice("Résolution locale en cours…");
    try {
      const result = await resolveIdentities(workspaceId, true);
      setNotice(
        result.status === "CANCELLED"
          ? "Résolution annulée ; les résultats déjà validés restent cohérents."
          : `${result.occurrencesProcessed.toLocaleString()} occurrence(s) traitée(s), ${result.candidatesCreated.toLocaleString()} candidate(s) créée(s).`,
      );
      setRefresh((value) => value + 1);
    } catch (reason) {
      setError(getErrorMessage(reason));
      setNotice(null);
    } finally {
      setResolving(false);
    }
  }

  async function cancelResolution() {
    setError(null);
    try {
      const requested = await cancelIdentityResolution(workspaceId);
      if (requested) {
        setNotice("Annulation demandée ; l’état local déjà validé sera conservé.");
      } else {
        setResolving(false);
        setNotice("Aucune résolution manuelle active à annuler.");
      }
    } catch (reason) {
      setError(getErrorMessage(reason));
    }
  }

  return (
    <div className="identity-review-surface">
      <div className="surface-heading">
        <div>
          <span className="step">Relations locales</span>
          <h2>Identités et projets à vérifier</h2>
          <p>
            Les rapprochements sont locaux, explicables et limités à la base sémantique.
            Aucun fichier n’est déplacé, renommé ou modifié.
          </p>
        </div>
        <div className="identity-resolution-actions">
          <div className="review-total" aria-live="polite">
            <strong>{page?.total.toLocaleString() ?? "—"}</strong>
            <span>groupes</span>
          </div>
          {resolving || resolution?.phase === "RUNNING" ? (
            <button
              type="button"
              className="danger-outline"
              onClick={() => void cancelResolution()}
            >
              Annuler
            </button>
          ) : (
            <button type="button" onClick={() => void runResolution()}>
              Relancer localement
            </button>
          )}
        </div>
      </div>

      {resolving || resolution?.phase === "RUNNING" ? (
        <div className="identity-progress" role="status">
          <strong>Résolution en cours</strong>
          <span>{(resolution?.filesConsidered ?? 0).toLocaleString()} fichier(s)</span>
          <span>
            {(resolution?.occurrencesProcessed ?? 0).toLocaleString()} occurrence(s)
          </span>
          <span>
            {(resolution?.comparisons ?? 0).toLocaleString()} comparaison(s) bornée(s)
          </span>
        </div>
      ) : null}

      <div className="review-toolbar">
        <label className="status-filter">
          <span>État</span>
          <select
            value={status}
            onChange={(event) => {
              setOffset(0);
              setStatus(event.target.value as typeof status);
            }}
          >
            <option value="needs_review">À vérifier</option>
            <option value="resolved">Résolus</option>
            <option value="all">Tous</option>
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
      {loading ? <p className="view-note">Chargement des relations locales…</p> : null}
      {!loading && !error && page?.items.length === 0 ? (
        <div className="empty-state">
          <strong>Aucune relation dans cette vue</strong>
          <p>UNKNOWN et les séparations confirmées restent des résultats valides.</p>
        </div>
      ) : null}

      <div className="identity-review-list" aria-busy={loading}>
        {page?.items.map((group) => (
          <article
            key={group.reviewGroupId}
            className={`identity-review-group identity-review-group--${group.reviewReason.toLowerCase()}`}
          >
            <div className="identity-group-heading">
              <div>
                <span className="semantic-label">{reviewReason(group.reviewReason)}</span>
                <h3>{group.title}</h3>
                <p>{group.explanation}</p>
              </div>
              <div className="identity-group-metrics">
                <span>{group.fileCount.toLocaleString()} fichiers</span>
                <span>{group.occurrenceCount.toLocaleString()} occurrences</span>
                <strong>{scoreLabel(group.maxScore)}</strong>
              </div>
            </div>

            {group.candidates.map((candidate) => {
              const working = workingId === candidate.candidateId;
              return (
                <section className="identity-candidate-card" key={candidate.candidateId}>
                  <div className="identity-pair">
                    <IdentitySide
                      label={
                        candidate.left.identityType === "PROJECT" ? "Projet candidat" : "Identité A"
                      }
                      name={candidate.left.displayName}
                      files={candidate.left.fileCount}
                      roles={candidate.left.roles}
                      onInspect={() => onOpenIdentity(candidate.left.identityId)}
                    />
                    <span className="identity-pair-arrow" aria-hidden="true">
                      ↔
                    </span>
                    <IdentitySide
                      label={
                        candidate.right.identityType === "PROJECT"
                          ? "Projet candidat"
                          : "Identité B"
                      }
                      name={candidate.right.displayName}
                      files={candidate.right.fileCount}
                      roles={candidate.right.roles}
                      onInspect={() => onOpenIdentity(candidate.right.identityId)}
                    />
                  </div>

                  <div className="identity-evidence-list">
                    <h4>Pourquoi ce rapprochement ?</h4>
                    {candidate.evidence.map((evidence, index) => (
                      <div
                        className={`identity-evidence identity-evidence--${evidence.polarity.toLowerCase()}`}
                        key={`${evidence.evidenceType}-${index}`}
                      >
                        <span aria-hidden="true">
                          {evidence.polarity === "CONFLICTS" ? "✕" : "✓"}
                        </span>
                        <div>
                          <strong>{evidence.explanation}</strong>
                          <small>
                            {friendly(evidence.strength)} · {evidence.leftValue}
                            {evidence.leftValue !== evidence.rightValue
                              ? ` ↔ ${evidence.rightValue}`
                              : ""}
                          </small>
                        </div>
                      </div>
                    ))}
                  </div>

                  <div className="identity-candidate-footer">
                    <span>
                      Score de politique : {Math.round(candidate.score * 100)} % ·{" "}
                      {friendly(candidate.policyDecision)}
                    </span>
                    {group.status === "NEEDS_REVIEW" &&
                    ["CANDIDATE", "CONFLICTING"].includes(candidate.status) ? (
                      <div className="review-actions">
                        <button
                          type="button"
                          disabled={workingId !== null}
                          onClick={() => void decide(candidate, "keep_separate")}
                        >
                          Garder séparées
                        </button>
                        <button
                          type="button"
                          className="primary"
                          disabled={workingId !== null}
                          onClick={() => void decide(candidate, "confirm")}
                        >
                          {working ? "Enregistrement…" : "Confirmer identiques"}
                        </button>
                      </div>
                    ) : (
                      <span className={`review-state review-state--${candidate.status.toLowerCase()}`}>
                        {friendly(candidate.status)}
                      </span>
                    )}
                  </div>
                </section>
              );
            })}
          </article>
        ))}
      </div>

      {page && (offset > 0 || page.hasMore) ? (
        <nav className="pagination" aria-label="Pages des relations à vérifier">
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

function IdentitySide({
  label,
  name,
  files,
  roles,
  onInspect,
}: {
  label: string;
  name: string;
  files: number;
  roles: string[];
  onInspect: () => void;
}) {
  return (
    <div className="identity-side">
      <span>{label}</span>
      <strong>{name}</strong>
      <small>
        {files.toLocaleString()} fichier(s)
        {roles.length > 0 ? ` · ${roles.map(friendly).join(", ")}` : ""}
      </small>
      <button type="button" onClick={onInspect}>
        Inspecter les fichiers
      </button>
    </div>
  );
}

function reviewReason(value: string): string {
  const labels: Record<string, string> = {
    POSSIBLE_DUPLICATE_IDENTITY: "Identité possiblement dupliquée",
    CONFLICTING_IDENTITY_EVIDENCE: "Éléments contradictoires",
    AMBIGUOUS_PROJECT_MATCH: "Projet ambigu",
    AMBIGUOUS_PERSON_MATCH: "Personne ambiguë",
  };
  return labels[value] ?? friendly(value);
}

function scoreLabel(value: number): string {
  if (value >= 0.97) {
    return "Très élevé";
  }
  if (value >= 0.75) {
    return "Élevé";
  }
  if (value >= 0.5) {
    return "À vérifier";
  }
  return "Inconnu";
}

function friendly(value: string): string {
  const labels: Record<string, string> = {
    CUSTOMER: "Client",
    SUPPLIER: "Fournisseur",
    VERY_STRONG: "Très fort",
    STRONG: "Fort",
    MEDIUM: "Moyen",
    WEAK: "Faible",
    CONFLICTING: "Contradictoire",
    REVIEW: "Revue requise",
    KEEP_SEPARATE: "Garder séparées",
    USER_CONFIRMED: "Confirmé par vous",
    USER_REJECTED: "Séparé par vous",
    AUTO_LINKED: "Relié automatiquement",
  };
  return labels[value] ?? value.replace(/_/g, " ").toLowerCase();
}
