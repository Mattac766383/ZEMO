import { useEffect, useMemo, useRef, useState } from "react";
import {
  getErrorMessage,
  getFileDetail,
  storeSemanticCorrection,
} from "./api";
import type { LocalFileDetail, SemanticField } from "./types";
import "./FileDetailPanelV2.css";

interface FileDetailPanelProps {
  fileId: string;
  onClose: () => void;
  onOpenIdentity?: (identityId: string) => void;
}

export function FileDetailPanel({
  fileId,
  onClose,
  onOpenIdentity,
}: FileDetailPanelProps) {
  const [detail, setDetail] = useState<LocalFileDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editingField, setEditingField] = useState<string | null>(null);
  const [correctionValue, setCorrectionValue] = useState("");
  const [savingField, setSavingField] = useState<string | null>(null);
  const viewToken = useRef(0);

  useEffect(() => {
    viewToken.current += 1;
    let active = true;
    setDetail(null);
    setError(null);
    setEditingField(null);
    setCorrectionValue("");
    setSavingField(null);
    void getFileDetail(fileId)
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
  }, [fileId]);

  async function saveCorrection(
    field: SemanticField,
    action: "confirm" | "correct",
  ) {
    setError(null);
    setSavingField(field.fieldKey);
    const token = viewToken.current;
    try {
      await storeSemanticCorrection(
        fileId,
        field.fieldKey.toLowerCase(),
        action,
        action === "correct" ? correctionValue : undefined,
      );
      const refreshed = await getFileDetail(fileId);
      if (viewToken.current === token) {
        setDetail(refreshed);
        setEditingField(null);
        setCorrectionValue("");
      }
    } catch (reason) {
      if (viewToken.current === token) {
        setError(getErrorMessage(reason));
      }
    } finally {
      if (viewToken.current === token) {
        setSavingField(null);
      }
    }
  }

  const intelligence = useMemo(
    () => (detail ? deriveFileIntelligence(detail) : null),
    [detail],
  );

  return (
    <section className="file-detail-panel" aria-labelledby="file-detail-title">
      <div className="surface-heading">
        <div>
          <span className="step">Ce que ZEMO sait</span>
        </div>
        <button type="button" onClick={onClose} aria-label="Fermer les détails du fichier">
          Fermer
        </button>
      </div>

      {error ? (
        <p className="inline-error" role="alert">
          {error}
        </p>
      ) : null}
      {!detail && !error ? (
        <p className="view-note" aria-live="polite">
          Chargement des informations locales…
        </p>
      ) : null}

      {detail && intelligence ? (
        <>
          <div className="file-intelligence-hero">
            <div className="file-intelligence-hero-main">
              <div className="file-intelligence-title-row">
                <div className="file-intelligence-icon" aria-hidden="true">
                  {fileIcon(intelligence.documentType, detail.extension)}
                </div>
                <div>
                  <h2 id="file-detail-title">{detail.filename}</h2>
                  <p className="file-intelligence-subtitle">
                    {intelligence.documentTypeLabel}
                    {intelligence.contextLabel ? ` · ${intelligence.contextLabel}` : ""}
                    {intelligence.confidencePercent != null
                      ? ` · ${intelligence.confidencePercent} % compris`
                      : ""}
                  </p>
                </div>
              </div>
              <span
                className={`file-intelligence-status file-intelligence-status--${intelligence.statusTone}`}
              >
                {intelligence.statusLabel}
              </span>
            </div>
            <p className="file-intelligence-summary">{intelligence.summary}</p>
            <div className="file-intelligence-quick-facts">
              <span>{formatBytes(detail.byteSize)}</span>
              <span>Modifié {formatTimestamp(detail.modifiedAt)}</span>
              {detail.duplicate ? <span>Doublon exact connu</span> : null}
              {detail.reviewItems.some((item) => item.status.toUpperCase() === "NEEDS_REVIEW") ? (
                <span>À vérifier</span>
              ) : null}
            </div>
          </div>

          <section className="file-intelligence-section" aria-labelledby="file-understanding-title">
            <div className="file-intelligence-section-heading">
              <div>
                <h3 id="file-understanding-title">Ce que ZEMO a compris</h3>
                <p>
                  Informations locales et explicables. Une confiance machine n’est jamais une
                  confirmation humaine.
                </p>
              </div>
            </div>

            {intelligence.visibleFields.length > 0 ? (
              <div className="file-intelligence-field-grid">
                {intelligence.visibleFields.map((field) => (
                  <SemanticFieldCard
                    key={field.fieldId}
                    field={field}
                    editing={editingField === field.fieldKey}
                    correctionValue={correctionValue}
                    saving={savingField === field.fieldKey}
                    onCorrectionValue={setCorrectionValue}
                    onEdit={() => {
                      setEditingField(field.fieldKey);
                      setCorrectionValue(field.displayValue ?? "");
                    }}
                    onCancel={() => {
                      setEditingField(null);
                      setCorrectionValue("");
                    }}
                    onConfirm={() => void saveCorrection(field, "confirm")}
                    onCorrect={() => void saveCorrection(field, "correct")}
                  />
                ))}
              </div>
            ) : (
              <p className="file-intelligence-empty">
                ZEMO n’a pas encore suffisamment d’informations structurées sur ce fichier.
              </p>
            )}

            {detail.semanticAnalysis?.entities.length ? (
              <details>
                <summary>
                  Autres entités détectées ({detail.semanticAnalysis.entities.length})
                </summary>
                <ul className="file-intelligence-entity-list">
                  {detail.semanticAnalysis.entities.map((entity) => (
                    <li key={entity.entityId}>
                      {friendlyFieldKey(entity.entityType)} · {entity.normalizedValue} ·{" "}
                      {Math.round(entity.confidence * 100)} %
                    </li>
                  ))}
                </ul>
              </details>
            ) : null}
          </section>

          <section className="file-intelligence-section" aria-labelledby="file-relations-title">
            <div className="file-intelligence-section-heading">
              <div>
                <h3 id="file-relations-title">Relations</h3>
                <p>Projets, personnes et organisations reliés à ce fichier par l’index local.</p>
              </div>
            </div>
            {detail.relationships.length > 0 ? (
              <div className="file-intelligence-relations">
                {detail.relationships.map((relationship) => (
                  <article
                    className="file-intelligence-relation"
                    key={relationship.relationshipId}
                  >
                    <div className="file-intelligence-relation-head">
                      <div>
                        <span className="file-intelligence-relation-label">
                          {friendlyRelationship(relationship.relationshipType)}
                        </span>
                        <strong>{relationship.displayName}</strong>
                        <small>
                          {friendlyStatus(relationship.status)} ·{" "}
                          {Math.round(relationship.confidence * 100)} % de confiance
                        </small>
                      </div>
                      {onOpenIdentity ? (
                        <button
                          type="button"
                          onClick={() => onOpenIdentity(relationship.identityId)}
                        >
                          Voir l’identité
                        </button>
                      ) : null}
                    </div>
                    {relationship.evidence.length > 0 ? (
                      <details>
                        <summary>Pourquoi ?</summary>
                        <ul>
                          {relationship.evidence.map((evidence, index) => (
                            <li key={`${relationship.relationshipId}-${index}`}>{evidence}</li>
                          ))}
                        </ul>
                      </details>
                    ) : null}
                  </article>
                ))}
              </div>
            ) : (
              <p className="file-intelligence-empty">
                Aucune relation inter-fichiers confirmée ou suffisamment fiable n’est disponible.
              </p>
            )}
          </section>

          <section className="file-intelligence-section" aria-labelledby="file-preview-title">
            <div className="file-intelligence-section-heading">
              <div>
                <h3 id="file-preview-title">Aperçu</h3>
                <p>Texte extrait localement, limité à l’aperçu sûr déjà indexé par ZEMO.</p>
              </div>
            </div>
            <pre className="file-intelligence-preview">
              {detail.textPreview || "Aucun texte exploitable n’a été extrait."}
            </pre>
          </section>

          <section className="file-intelligence-section" aria-labelledby="file-review-title">
            <div className="file-intelligence-section-heading">
              <div>
                <h3 id="file-review-title">Vérification</h3>
                <p>Points où ZEMO préfère demander un avis plutôt que d’inventer.</p>
              </div>
            </div>
            {detail.reviewItems.length === 0 ? (
              <p className="quiet-success">Aucun point ne demande votre attention.</p>
            ) : (
              <div className="file-intelligence-review-list">
                {detail.reviewItems.map((item) => (
                  <article key={item.reviewId} className="file-intelligence-review-item">
                    <strong>{item.explanation}</strong>
                    <p>
                      {friendlyReason(item.reason)} · extraction{" "}
                      {friendlyStatus(item.extractionStatus)} · {friendlyStatus(item.status)}
                    </p>
                    {item.technicalDetails ? (
                      <details>
                        <summary>Détails techniques</summary>
                        <p>{item.technicalDetails}</p>
                      </details>
                    ) : null}
                  </article>
                ))}
              </div>
            )}
          </section>

          <details className="file-intelligence-technical">
            <summary>Informations techniques du fichier</summary>
            <dl className="file-intelligence-technical-grid">
              <Detail label="Nom" value={detail.filename} />
              <Detail label="Emplacement relatif" value={detail.relativePath} mono />
              <Detail label="Extension" value={detail.extension ?? "Non disponible"} />
              <Detail
                label="Type détecté"
                value={detail.detectedType ?? "Non disponible"}
              />
              <Detail label="Taille" value={formatBytes(detail.byteSize)} />
              <Detail label="Créé" value={formatTimestamp(detail.createdAt)} />
              <Detail label="Modifié" value={formatTimestamp(detail.modifiedAt)} />
              <Detail
                label="Empreinte BLAKE3"
                value={detail.hash ?? "Non disponible"}
                mono
              />
              <Detail
                label="Doublon exact"
                value={detail.duplicate ? "Oui" : "Aucun doublon exact connu"}
              />
              <Detail label="Extraction" value={friendlyStatus(detail.extractionStatus)} />
              <Detail
                label="Extracteur"
                value={
                  detail.extractorType
                    ? `${detail.extractorType}${
                        detail.extractorVersion ? ` · ${detail.extractorVersion}` : ""
                      }`
                    : "Non disponible"
                }
              />
              <Detail label="OCR" value={friendlyOcr(detail.ocrStatus)} />
              <Detail
                label="Texte extrait"
                value={`${detail.characterCount.toLocaleString()} caractères`}
              />
              <Detail
                label="Analyse sémantique"
                value={
                  detail.semanticAnalysis
                    ? `${friendlyStatus(detail.semanticAnalysis.status)} · ${detail.semanticAnalysis.analyzerVersion}`
                    : "Non disponible"
                }
              />
              <Detail
                label="Langue comprise"
                value={detail.semanticAnalysis?.language?.toUpperCase() ?? "Non disponible"}
              />
            </dl>
          </details>
        </>
      ) : null}
    </section>
  );
}

function SemanticFieldCard({
  field,
  editing,
  correctionValue,
  saving,
  onCorrectionValue,
  onEdit,
  onCancel,
  onConfirm,
  onCorrect,
}: {
  field: SemanticField;
  editing: boolean;
  correctionValue: string;
  saving: boolean;
  onCorrectionValue: (value: string) => void;
  onEdit: () => void;
  onCancel: () => void;
  onConfirm: () => void;
  onCorrect: () => void;
}) {
  const options = correctionOptions(field.fieldKey);
  const human = isHumanField(field);
  const display = field.displayValue
    ? friendlySemanticValue(field.fieldKey, field.displayValue)
    : field.status.toUpperCase() === "CONFLICTING"
      ? "Valeurs contradictoires"
      : "Non déterminé";

  return (
    <article className="file-intelligence-field">
      <div className="file-intelligence-field-main">
        <div>
          <span className="file-intelligence-field-label">
            {friendlyFieldKey(field.fieldKey)}
          </span>
          <strong>{display}</strong>
          <small>
            {human
              ? field.userState?.toUpperCase().includes("CORRECT")
                ? "Corrigé par vous"
                : "Confirmé par vous"
              : confidenceLabel(field.confidence)}
          </small>
        </div>
        <span
          className={`file-intelligence-confidence${human ? " file-intelligence-confidence--human" : ""}`}
        >
          {human ? "HUMAIN" : `${Math.round(field.confidence * 100)} %`}
        </span>
      </div>

      {human && field.machineDisplayValue ? (
        <p className="file-intelligence-machine-value">
          Valeur machine précédente :{" "}
          {friendlySemanticValue(field.fieldKey, field.machineDisplayValue)}
        </p>
      ) : null}

      {field.evidence.length > 0 ? (
        <details>
          <summary>Pourquoi ?</summary>
          {field.evidence.map((evidence, index) => (
            <div
              className="file-intelligence-evidence"
              key={`${evidence.evidenceType}-${evidence.startOffset ?? index}`}
            >
              <q>{evidence.exactText}</q>
              <p>
                {evidence.explanation}
                {evidence.pageNumber ? ` · page ${evidence.pageNumber}` : ""}
                {evidence.sheetName ? ` · feuille ${evidence.sheetName}` : ""}
                {evidence.slideNumber ? ` · diapositive ${evidence.slideNumber}` : ""}
              </p>
            </div>
          ))}
        </details>
      ) : null}

      {field.candidates.length > 0 ? (
        <details>
          <summary>Autres interprétations</summary>
          <ul>
            {field.candidates.slice(0, 6).map((candidate, index) => (
              <li key={`${candidate.displayValue}-${index}`}>
                {friendlySemanticValue(field.fieldKey, candidate.displayValue)} ·{" "}
                {Math.round(candidate.confidence * 100)} %
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      {editing ? (
        <div className="file-intelligence-editor">
          <label>
            Correction
            {options ? (
              <select
                value={correctionValue.toLowerCase()}
                onChange={(event) => onCorrectionValue(event.target.value)}
                disabled={saving}
              >
                <option value="">Choisir…</option>
                {options.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            ) : (
              <input
                value={correctionValue}
                maxLength={256}
                onChange={(event) => onCorrectionValue(event.target.value)}
                disabled={saving}
              />
            )}
          </label>
          <div className="file-intelligence-actions">
            <button type="button" onClick={onCancel} disabled={saving}>
              Annuler
            </button>
            <button
              className="primary"
              type="button"
              onClick={onCorrect}
              disabled={saving || correctionValue.trim().length === 0}
            >
              {saving ? "Enregistrement…" : "Enregistrer la correction"}
            </button>
          </div>
        </div>
      ) : (
        <div className="file-intelligence-actions">
          {field.displayValue && !human ? (
            <button type="button" onClick={onConfirm} disabled={saving}>
              Confirmer
            </button>
          ) : null}
          <button type="button" onClick={onEdit} disabled={saving}>
            Corriger
          </button>
        </div>
      )}
    </article>
  );
}

function Detail({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div>
      <dt>{label}</dt>
      <dd className={mono ? "is-mono" : undefined}>{value}</dd>
    </div>
  );
}

type FileIntelligence = {
  documentType: string | null;
  documentTypeLabel: string;
  contextLabel: string | null;
  confidencePercent: number | null;
  statusLabel: string;
  statusTone: "good" | "partial" | "review" | "unknown";
  summary: string;
  visibleFields: SemanticField[];
};

function deriveFileIntelligence(detail: LocalFileDetail): FileIntelligence {
  const fields = detail.semanticAnalysis?.fields ?? [];
  const visibleFields = fields.filter((field) => {
    if (!field.displayValue?.trim()) {
      return false;
    }
    const status = field.status.toUpperCase();
    return !["UNKNOWN"].includes(status);
  });
  const documentType = fieldValue(fields, ["document_type"]);
  const context = fieldValue(fields, ["context"]);
  const confidences = visibleFields
    .filter((field) => !isHumanField(field))
    .map((field) => field.confidence)
    .filter((confidence) => Number.isFinite(confidence));
  const confidencePercent =
    confidences.length > 0
      ? Math.round(
          (confidences.reduce((sum, confidence) => sum + confidence, 0) /
            confidences.length) *
            100,
        )
      : visibleFields.some(isHumanField)
        ? 100
        : null;
  const needsReview = detail.reviewItems.some(
    (item) => item.status.toUpperCase() === "NEEDS_REVIEW",
  );
  const semanticStatus = detail.semanticAnalysis?.status.toUpperCase();
  const hasAmbiguous = visibleFields.some((field) =>
    ["AMBIGUOUS", "CONFLICTING"].includes(field.status.toUpperCase()),
  );

  let statusLabel = "Non analysé";
  let statusTone: FileIntelligence["statusTone"] = "unknown";
  if (needsReview || hasAmbiguous) {
    statusLabel = "À vérifier";
    statusTone = "review";
  } else if (!detail.semanticAnalysis) {
    statusLabel = "Non analysé";
    statusTone = "unknown";
  } else if (
    semanticStatus === "PARTIAL" ||
    detail.extractionStatus?.toUpperCase() === "PARTIAL" ||
    confidencePercent === null ||
    confidencePercent < 80
  ) {
    statusLabel = "Partiellement compris";
    statusTone = "partial";
  } else {
    statusLabel = "Compris";
    statusTone = "good";
  }

  return {
    documentType,
    documentTypeLabel: documentType
      ? friendlySemanticValue("document_type", documentType)
      : humanDetectedType(detail.detectedType, detail.extension),
    contextLabel: context ? friendlySemanticValue("context", context) : null,
    confidencePercent,
    statusLabel,
    statusTone,
    summary: buildLocalSummary(detail, fields),
    visibleFields: prioritizeFields(visibleFields),
  };
}

function buildLocalSummary(detail: LocalFileDetail, fields: SemanticField[]): string {
  const documentType = fieldValue(fields, ["document_type"]);
  const context = fieldValue(fields, ["context"]);
  const supplier =
    relatedName(detail, ["FILE_SUPPLIER", "SUPPLIER"]) ??
    fieldValue(fields, ["supplier", "supplier_candidate", "issuer"]);
  const customer =
    relatedName(detail, ["FILE_CUSTOMER", "CUSTOMER", "PROJECT_CUSTOMER"]) ??
    fieldValue(fields, ["customer", "customer_candidate"]);
  const project =
    relatedName(detail, ["FILE_PROJECT", "DOCUMENT_PROJECT", "PROJECT"]) ??
    fieldValue(fields, ["project", "project_reference_candidate"]);
  const date = fieldValue(fields, ["document_date", "issue_date", "date"]);
  const amount = fieldValue(fields, ["total", "amount"]);
  const currency = fieldValue(fields, ["currency"]);

  const subject = documentType
    ? friendlySemanticValue("document_type", documentType)
    : humanDetectedType(detail.detectedType, detail.extension);
  const pieces: string[] = [];
  let first = subject;
  if (context) {
    first += ` ${friendlySemanticValue("context", context).toLowerCase()}`;
  }
  if (supplier) {
    first += ` lié${subject.toLowerCase().startsWith("facture") ? "e" : ""} à ${supplier}`;
  }
  pieces.push(`${first}.`);
  if (customer) {
    pieces.push(`Client détecté : ${customer}.`);
  }
  if (project) {
    pieces.push(`Projet : ${project}.`);
  }
  if (date) {
    pieces.push(`Date détectée : ${date}.`);
  }
  if (amount) {
    pieces.push(`Montant détecté : ${amount}${currency ? ` ${currency}` : ""}.`);
  }
  if (pieces.length === 1 && !documentType && !context && !supplier && !customer && !project) {
    return detail.characterCount > 0
      ? "ZEMO a extrait du contenu localement, mais n’a pas encore suffisamment d’informations structurées pour résumer ce fichier."
      : "ZEMO n’a pas encore suffisamment d’informations pour résumer ce fichier.";
  }
  return pieces.join(" ");
}

function relatedName(detail: LocalFileDetail, relationshipTypes: string[]): string | null {
  const accepted = new Set(relationshipTypes.map((value) => value.toUpperCase()));
  return (
    detail.relationships.find((relationship) =>
      accepted.has(relationship.relationshipType.toUpperCase()),
    )?.displayName ?? null
  );
}

function prioritizeFields(fields: SemanticField[]): SemanticField[] {
  const priority = [
    "document_type",
    "context",
    "supplier",
    "supplier_candidate",
    "issuer",
    "customer",
    "customer_candidate",
    "project",
    "project_reference_candidate",
    "invoice_number",
    "quote_number",
    "document_number",
    "document_date",
    "issue_date",
    "due_date",
    "total",
    "amount",
    "currency",
    "address",
    "person",
    "organization",
  ];
  const rank = new Map(priority.map((key, index) => [key, index]));
  return [...fields].sort((left, right) => {
    const leftRank = rank.get(left.fieldKey.toLowerCase()) ?? priority.length;
    const rightRank = rank.get(right.fieldKey.toLowerCase()) ?? priority.length;
    return leftRank - rightRank || left.fieldKey.localeCompare(right.fieldKey);
  });
}

function fieldValue(fields: SemanticField[], keys: string[]): string | null {
  const accepted = new Set(keys.map((key) => key.toLowerCase()));
  return (
    fields.find(
      (field) => accepted.has(field.fieldKey.toLowerCase()) && Boolean(field.displayValue?.trim()),
    )?.displayValue?.trim() ?? null
  );
}

function isHumanField(field: SemanticField): boolean {
  return field.valueSource.toUpperCase().includes("USER");
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
    const date = /^\d+$/.test(value)
      ? new Date(Number(BigInt(value) / 1_000_000n))
      : new Date(value);
    return Number.isNaN(date.getTime()) ? "Non disponible" : date.toLocaleString();
  } catch {
    return "Non disponible";
  }
}

function friendlyStatus(value?: string | null): string {
  if (!value) {
    return "Non analysé";
  }
  const labels: Record<string, string> = {
    NEEDS_REVIEW: "À vérifier",
    RESOLVED: "Résolu",
    IGNORED: "Ignoré",
    SUCCESS: "Réussie",
    PARTIAL: "Partielle",
    FAILED: "Échec",
    UNKNOWN: "Inconnue",
    COMPLETED: "Terminée",
    CANCELLED: "Annulée",
    UNSUPPORTED: "Non pris en charge",
    SKIPPED: "Non analysé",
    CONFIRMED: "Confirmé",
    INFERRED: "Inféré",
    AMBIGUOUS: "Ambigu",
    CONFLICTING: "Contradictoire",
  };
  return labels[value.toUpperCase()] ?? value.replace(/_/g, " ").toLowerCase();
}

function friendlyOcr(value?: string | null): string {
  const labels: Record<string, string> = {
    USED: "Reconnaissance locale utilisée",
    NOT_USED: "Non utilisée",
    UNAVAILABLE: "Reconnaissance locale indisponible",
  };
  return value ? (labels[value.toUpperCase()] ?? value) : "Non analysé";
}

function friendlyReason(value: string): string {
  return value.replace(/_/g, " ").toLowerCase();
}

function friendlyRelationship(value: string): string {
  const labels: Record<string, string> = {
    FILE_SUPPLIER: "Fournisseur",
    FILE_CUSTOMER: "Client",
    FILE_PROJECT: "Projet",
    DOCUMENT_PROJECT: "Projet",
    PROJECT_CUSTOMER: "Projet → client",
  };
  return labels[value.toUpperCase()] ?? friendlyReason(value);
}

function friendlyFieldKey(value: string): string {
  const labels: Record<string, string> = {
    DOCUMENT_TYPE: "Type de document",
    CONTEXT: "Contexte",
    SUPPLIER: "Fournisseur",
    SUPPLIER_CANDIDATE: "Fournisseur",
    CUSTOMER: "Client",
    CUSTOMER_CANDIDATE: "Client",
    ISSUER: "Émetteur",
    PROJECT: "Projet",
    PROJECT_REFERENCE_CANDIDATE: "Projet / référence",
    INVOICE_NUMBER: "Numéro de facture",
    QUOTE_NUMBER: "Numéro de devis",
    DOCUMENT_NUMBER: "Numéro de document",
    ISSUE_DATE: "Date d’émission",
    DUE_DATE: "Date d’échéance",
    EXPIRATION_DATE: "Date d’expiration",
    DOCUMENT_DATE: "Date du document",
    SUBTOTAL: "Sous-total",
    TAX: "Taxe",
    TOTAL: "Total",
    AMOUNT: "Montant",
    CURRENCY: "Devise",
    PURCHASE_ORDER_REFERENCE: "Référence de commande",
    CONTRACT_PARTIES: "Parties au contrat",
    CONTRACT_TITLE: "Titre du contrat",
    CONTRACT_TYPE: "Type de contrat",
    COMPANY_IDENTIFIER: "Identifiant d’entreprise",
    PERSON: "Personne",
    ORGANIZATION: "Organisation",
    EMAIL: "Adresse e-mail",
    PHONE: "Téléphone",
    ADDRESS: "Adresse",
    DATE: "Date",
    YEAR: "Année",
    SIRET_OR_COMPANY_ID: "Identifiant d’entreprise",
  };
  return labels[value.toUpperCase()] ?? friendlyReason(value);
}

function friendlySemanticValue(fieldKey: string, value: string): string {
  if (fieldKey.toUpperCase() === "DOCUMENT_TYPE") {
    const labels: Record<string, string> = {
      invoice: "Facture",
      quote: "Devis",
      contract: "Contrat",
      purchase_order: "Bon de commande",
      delivery_note: "Bon de livraison",
      bank_statement: "Relevé bancaire",
      tax_document: "Document fiscal",
      payslip: "Bulletin de paie",
      employment_contract: "Contrat de travail",
      insurance_document: "Document d’assurance",
      legal_document: "Document juridique",
      administrative_document: "Document administratif",
      receipt: "Reçu",
      report: "Rapport",
      letter: "Lettre",
      cv: "CV",
      photo: "Photo",
      video: "Vidéo",
      spreadsheet: "Tableur",
      presentation: "Présentation",
      archive: "Archive",
      other: "Autre",
      unknown: "Inconnu",
    };
    return labels[value.toLowerCase()] ?? value;
  }
  if (fieldKey.toUpperCase() === "CONTEXT") {
    const labels: Record<string, string> = {
      personal: "Personnel",
      business: "Professionnel",
      professional: "Professionnel",
      mixed: "Mixte",
      unknown: "Inconnu",
    };
    return labels[value.toLowerCase()] ?? value;
  }
  return value;
}

function confidenceLabel(value: number): string {
  if (value >= 0.95) {
    return "Très fiable";
  }
  if (value >= 0.85) {
    return "Fiable";
  }
  if (value >= 0.65) {
    return "Confiance moyenne";
  }
  return "À vérifier";
}

function correctionOptions(
  fieldKey: string,
): Array<{ value: string; label: string }> | null {
  if (fieldKey.toUpperCase() === "CONTEXT") {
    return [
      { value: "personal", label: "Personnel" },
      { value: "business", label: "Professionnel" },
      { value: "mixed", label: "Mixte" },
      { value: "unknown", label: "Inconnu" },
    ];
  }
  if (fieldKey.toUpperCase() !== "DOCUMENT_TYPE") {
    return null;
  }
  return [
    "invoice",
    "quote",
    "contract",
    "purchase_order",
    "delivery_note",
    "bank_statement",
    "tax_document",
    "payslip",
    "employment_contract",
    "insurance_document",
    "legal_document",
    "administrative_document",
    "receipt",
    "report",
    "letter",
    "cv",
    "photo",
    "video",
    "spreadsheet",
    "presentation",
    "archive",
    "other",
    "unknown",
  ].map((value) => ({
    value,
    label: friendlySemanticValue("DOCUMENT_TYPE", value),
  }));
}

function humanDetectedType(detectedType?: string | null, extension?: string | null): string {
  const raw = detectedType?.trim() || extension?.replace(/^\./, "").trim();
  if (!raw) {
    return "Type non déterminé";
  }
  const labels: Record<string, string> = {
    pdf: "PDF",
    document: "Document",
    image: "Image",
    video: "Vidéo",
    audio: "Audio",
    spreadsheet: "Tableur",
    presentation: "Présentation",
    archive: "Archive",
  };
  return labels[raw.toLowerCase()] ?? raw;
}

function fileIcon(documentType: string | null, extension?: string | null): string {
  const value = (documentType ?? extension ?? "").toLowerCase();
  if (value.includes("invoice") || value.includes("receipt")) {
    return "€";
  }
  if (value.includes("photo") || /png|jpg|jpeg|heic|webp/.test(value)) {
    return "▧";
  }
  if (value.includes("spreadsheet") || /xlsx|xls|csv/.test(value)) {
    return "▦";
  }
  if (value.includes("presentation") || /pptx|ppt/.test(value)) {
    return "▤";
  }
  if (value.includes("contract")) {
    return "§";
  }
  return "▱";
}
