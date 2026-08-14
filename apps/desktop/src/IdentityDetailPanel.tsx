import { useEffect, useRef, useState } from "react";
import {
  getErrorMessage,
  getIdentityDetail,
  unlinkIdentityOccurrence,
} from "./api";
import type { IdentityDetail } from "./types";

interface IdentityDetailPanelProps {
  identityId: string;
  onClose: () => void;
  onOpenFile: (fileId: string) => void;
  onOpenIdentity?: (identityId: string) => void;
}

export function IdentityDetailPanel({
  identityId,
  onClose,
  onOpenFile,
  onOpenIdentity,
}: IdentityDetailPanelProps) {
  const [detail, setDetail] = useState<IdentityDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pendingUnlink, setPendingUnlink] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const viewToken = useRef(0);

  useEffect(() => {
    viewToken.current += 1;
    let active = true;
    setDetail(null);
    setError(null);
    setPendingUnlink(null);
    setWorking(false);
    void getIdentityDetail(identityId)
      .then((next) => {
        if (active) {
          setDetail(next);
        }
      })
      .catch((reason) => {
        if (active) {
          setError(getErrorMessage(reason));
        }
      });
    return () => {
      active = false;
    };
  }, [identityId]);

  async function unlinkOccurrence(occurrenceId: string) {
    if (pendingUnlink !== occurrenceId) {
      setPendingUnlink(occurrenceId);
      return;
    }
    setWorking(true);
    setError(null);
    const token = viewToken.current;
    const effectiveIdentityId = detail?.identity.identityId ?? identityId;
    try {
      await unlinkIdentityOccurrence(
        effectiveIdentityId,
        occurrenceId,
        "Occurrence séparée depuis le détail d’identité",
      );
      const refreshed = await getIdentityDetail(effectiveIdentityId);
      if (viewToken.current === token) {
        setDetail(refreshed);
        setPendingUnlink(null);
      }
    } catch (reason) {
      if (viewToken.current === token) {
        setError(getErrorMessage(reason));
      }
    } finally {
      if (viewToken.current === token) {
        setWorking(false);
      }
    }
  }

  return (
    <section className="identity-detail-panel" aria-labelledby="identity-detail-title">
      <div className="surface-heading">
        <div>
          <span className="step">Identité locale</span>
          <h2 id="identity-detail-title">{detail?.identity.displayName ?? "Chargement…"}</h2>
        </div>
        <button type="button" onClick={onClose} aria-label="Fermer le détail de l’identité">
          Fermer
        </button>
      </div>

      {error ? (
        <p className="inline-error" role="alert">
          {error}
        </p>
      ) : null}
      {!detail && !error ? <p className="view-note">Chargement de l’identité locale…</p> : null}

      {detail ? (
        <>
          <dl className="detail-grid">
            <Detail label="Type" value={friendly(detail.identity.identityType)} />
            <Detail
              label="État"
              value={
                detail.identity.userLocked
                  ? "Structure confirmée par vous"
                  : friendly(detail.identity.resolutionStatus)
              }
            />
            <Detail
              label="Rôles observés"
              value={detail.identity.roles.map(friendly).join(", ") || "Aucun"}
            />
            <Detail label="Fichiers" value={detail.identity.fileCount.toLocaleString()} />
            <Detail
              label="Occurrences"
              value={detail.identity.occurrenceCount.toLocaleString()}
            />
            <Detail
              label="Score de politique"
              value={`${Math.round(detail.identity.confidence * 100)} % (non probabiliste)`}
            />
            <Detail label="Résolveur" value={detail.resolverVersion} />
          </dl>

          <div className="identity-detail-grid">
            <section className="detail-section">
              <h3>Alias observés</h3>
              {detail.identity.aliases.length > 0 ? (
                <ul className="identity-plain-list">
                  {detail.identity.aliases.map((alias) => (
                    <li key={alias}>{alias}</li>
                  ))}
                </ul>
              ) : (
                <p className="view-note">Aucun alias supplémentaire.</p>
              )}
            </section>

            <section className="detail-section">
              <h3>Identifiants structurés</h3>
              {detail.identifiers.length > 0 ? (
                <dl className="identity-identifier-list">
                  {detail.identifiers.map((identifier) => (
                    <div key={`${identifier.kind}-${identifier.value}`}>
                      <dt>{friendly(identifier.kind)}</dt>
                      <dd>{identifier.value}</dd>
                    </div>
                  ))}
                </dl>
              ) : (
                <p className="view-note">Aucun identifiant fort observé.</p>
              )}
            </section>
          </div>

          {detail.projects.length > 0 ? (
            <section className="detail-section">
              <h3>Projets liés</h3>
              <div className="identity-chip-list">
                {detail.projects.map((project) => (
                  onOpenIdentity ? (
                    <button
                      type="button"
                      key={project.identityId}
                      onClick={() => onOpenIdentity(project.identityId)}
                    >
                      {project.displayName}
                    </button>
                  ) : (
                    <span key={project.identityId}>{project.displayName}</span>
                  )
                ))}
              </div>
            </section>
          ) : null}

          <section className="detail-section">
            <h3>Fichiers sources</h3>
            {detail.occurrencesTruncated ? (
              <p className="result-limit">
                Affichage limité à {detail.occurrences.length} sur{" "}
                {detail.occurrenceTotal} occurrences.
              </p>
            ) : null}
            <div className="identity-occurrence-list">
              {detail.occurrences.map((occurrence) => (
                <article key={occurrence.occurrenceId} className="identity-occurrence">
                  <div>
                    <strong>{occurrence.filename}</strong>
                    <code>{occurrence.relativePath}</code>
                    <small>
                      Valeur observée : {occurrence.originalValue} ·{" "}
                      {Math.round(occurrence.confidence * 100)} %
                    </small>
                  </div>
                  <div className="review-actions">
                    <button type="button" onClick={() => onOpenFile(occurrence.fileId)}>
                      Inspecter le fichier
                    </button>
                    {occurrence.active && detail.identity.occurrenceCount > 1 ? (
                      <button
                        type="button"
                        className={pendingUnlink === occurrence.occurrenceId ? "danger-outline" : ""}
                        disabled={working}
                        onClick={() => void unlinkOccurrence(occurrence.occurrenceId)}
                      >
                        {pendingUnlink === occurrence.occurrenceId
                          ? "Confirmer la séparation"
                          : "Séparer cette occurrence"}
                      </button>
                    ) : null}
                  </div>
                </article>
              ))}
            </div>
          </section>

          {detail.relationships.length > 0 ? (
            <section className="detail-section">
              <h3>Relations</h3>
              <ul className="identity-plain-list">
                {detail.relationships.map((relationship) => (
                  <li key={relationship.relationshipId}>
                    {friendly(relationship.relationshipType)} : {relationship.displayName} ·{" "}
                    {friendly(relationship.status)}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          <details className="detail-section identity-audit">
            <summary>Historique d’audit ({detail.auditEvents.length})</summary>
            {detail.auditEvents.length > 0 ? (
              <ul className="identity-plain-list">
                {detail.auditEvents.map((event, index) => (
                  <li key={`${event.createdAt}-${event.eventType}-${index}`}>
                    <strong>{friendly(event.eventType)}</strong>
                    <span>
                      {event.decisionSource === "USER" ? "Décision utilisateur" : "Résolveur local"}
                      {" · "}
                      {new Date(event.createdAt).toLocaleString()}
                    </span>
                    {event.reason ? <small>{event.reason}</small> : null}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="view-note">Aucun événement décisionnel.</p>
            )}
          </details>
        </>
      ) : null}
    </section>
  );
}

function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function friendly(value: string): string {
  const labels: Record<string, string> = {
    ORGANIZATION: "Organisation",
    PERSON: "Personne",
    PROJECT: "Projet",
    CUSTOMER: "Client",
    SUPPLIER: "Fournisseur",
    USER_CONFIRMED: "Confirmée par vous",
    AUTO_LINKED: "Reliée automatiquement",
    CANDIDATE: "Candidate",
    FILE_CUSTOMER: "Fichier → client",
    FILE_SUPPLIER: "Fichier → fournisseur",
    FILE_PROJECT: "Fichier → projet",
    PROJECT_CUSTOMER: "Projet → client",
  };
  return labels[value.toUpperCase()] ?? value.replace(/_/g, " ").toLowerCase();
}
