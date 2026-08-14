import { useEffect, useRef, useState } from "react";
import {
  getErrorMessage,
  getFileDetail,
  storeSemanticCorrection,
} from "./api";
import type { LocalFileDetail, SemanticField } from "./types";

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

  return (
    <section className="file-detail-panel" aria-labelledby="file-detail-title">
      <div className="surface-heading">
        <div>
          <span className="step">File detail</span>
          <h2 id="file-detail-title">{detail?.filename ?? "Chargement…"}</h2>
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
      {detail ? (
        <>
          <dl className="detail-grid">
            <Detail label="Nom" value={detail.filename} />
            <Detail label="Emplacement d’origine" value={detail.relativePath} mono />
            <Detail
              label="Type"
              value={detail.detectedType ?? detail.extension ?? "Type inconnu"}
            />
            <Detail label="Taille" value={formatBytes(detail.byteSize)} />
            <Detail label="Créé" value={formatTimestamp(detail.createdAt)} />
            <Detail label="Modifié" value={formatTimestamp(detail.modifiedAt)} />
            <Detail label="Empreinte BLAKE3" value={detail.hash ?? "Non disponible"} mono />
            <Detail
              label="Doublon exact"
              value={detail.duplicate ? "Oui" : "Aucun doublon exact connu"}
            />
            <Detail
              label="Extraction"
              value={friendlyStatus(detail.extractionStatus)}
            />
            <Detail
              label="Extracteur"
              value={
                detail.extractorType
                  ? `${detail.extractorType}${detail.extractorVersion ? ` · ${detail.extractorVersion}` : ""}`
                  : "Non disponible"
              }
            />
            <Detail label="OCR" value={friendlyOcr(detail.ocrStatus)} />
            <Detail
              label="Texte extrait"
              value={`${detail.characterCount.toLocaleString()} caractères`}
            />
          </dl>

          <div className="detail-section semantic-understanding">
            <div className="semantic-heading">
              <div>
                <h3>Compréhension</h3>
                <p>
                  Analyse locale et explicable. Une confiance machine n’est pas une
                  confirmation humaine.
                </p>
              </div>
              {detail.semanticAnalysis ? (
                <span
                  className={`review-state review-state--${detail.semanticAnalysis.status.toLowerCase()}`}
                >
                  {friendlyStatus(detail.semanticAnalysis.status)}
                </span>
              ) : null}
            </div>

            {detail.semanticAnalysis ? (
              <>
                <div className="semantic-summary">
                  <span>
                    Entrée {friendlyInputQuality(detail.semanticAnalysis.inputQualityStatus)}
                  </span>
                  <span>
                    Langue {detail.semanticAnalysis.language?.toUpperCase() ?? "inconnue"}
                  </span>
                  <span>
                    Analyseur {detail.semanticAnalysis.analyzerVersion}
                  </span>
                </div>
                <div className="semantic-field-list">
                  {detail.semanticAnalysis.fields.map((field) => (
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

                {detail.semanticAnalysis.entities.length > 0 ? (
                  <details className="semantic-entities">
                    <summary>
                      Entités détectées ({detail.semanticAnalysis.entities.length})
                    </summary>
                    <ul>
                      {detail.semanticAnalysis.entities.map((entity) => (
                        <li key={entity.entityId}>
                          <strong>{friendlyFieldKey(entity.entityType)}</strong>
                          <span>{entity.normalizedValue}</span>
                          <small>{confidenceLabel(entity.confidence)}</small>
                        </li>
                      ))}
                    </ul>
                  </details>
                ) : null}
              </>
            ) : (
              <p className="view-note">
                Aucune compréhension sémantique locale n’est encore disponible.
              </p>
            )}
          </div>

          <div className="detail-section file-relationships">
            <div className="semantic-heading">
              <div>
                <h3>Relations</h3>
                <p>Liens sémantiques locaux ; le fichier source reste inchangé.</p>
              </div>
            </div>
            {detail.relationships?.length > 0 ? (
              <div className="file-relationship-list">
                {detail.relationships.map((relationship) => (
                  <article className="file-relationship" key={relationship.relationshipId}>
                    <div>
                      <span className="semantic-label">
                        {friendlyRelationship(relationship.relationshipType)}
                      </span>
                      <strong>{relationship.displayName}</strong>
                      <small>
                        {friendlyStatus(relationship.status)} ·{" "}
                        {Math.round(relationship.confidence * 100)} % (score de politique)
                      </small>
                    </div>
                    {onOpenIdentity ? (
                      <button
                        type="button"
                        onClick={() => onOpenIdentity(relationship.identityId)}
                      >
                        Voir l’identité et les preuves
                      </button>
                    ) : null}
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
              <p className="view-note">Aucune relation inter-fichiers disponible.</p>
            )}
          </div>

          <div className="detail-section">
            <h3>Aperçu du texte</h3>
            <pre className="text-preview">
              {detail.textPreview || "Aucun texte exploitable n’a été extrait."}
            </pre>
          </div>

          <div className="detail-section">
            <h3>État de vérification</h3>
            {detail.reviewItems.length === 0 ? (
              <p className="quiet-success">Aucun point ne demande votre attention.</p>
            ) : (
              <div className="detail-review-list">
                {detail.reviewItems.map((item) => (
                  <article key={item.reviewId} className="detail-review">
                    <div>
                      <strong>{item.explanation}</strong>
                      <span className={`review-state review-state--${item.status.toLowerCase()}`}>
                        {friendlyStatus(item.status)}
                      </span>
                    </div>
                    <p>
                      Raison : {friendlyReason(item.reason)} · Extraction :{" "}
                      {friendlyStatus(item.extractionStatus)}
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
          </div>
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
  const display = field.displayValue
    ? friendlySemanticValue(field.fieldKey, field.displayValue)
    : field.status === "CONFLICTING"
      ? "Valeurs contradictoires"
      : "Inconnu";
  return (
    <article className={`semantic-field semantic-field--${field.status.toLowerCase()}`}>
      <div className="semantic-field-main">
        <div>
          <span className="semantic-label">{friendlyFieldKey(field.fieldKey)}</span>
          <strong>{display}</strong>
          {field.valueSource === "USER" ? (
            <small className="human-confirmation">
              {field.userState === "USER_CORRECTED"
                ? "Corrigé par vous"
                : "Confirmé par vous"}
            </small>
          ) : (
            <small>{confidenceLabel(field.confidence)}</small>
          )}
        </div>
        <span className={`semantic-confidence semantic-confidence--${confidenceTone(field)}`}>
          {field.valueSource === "USER"
            ? "HUMAIN"
            : `${Math.round(field.confidence * 100)} %`}
        </span>
      </div>

      {field.valueSource === "USER" && field.machineDisplayValue ? (
        <p className="machine-value">
          Valeur machine :{" "}
          {friendlySemanticValue(field.fieldKey, field.machineDisplayValue)}
        </p>
      ) : null}

      {field.candidates.length > 0 ? (
        <details className="semantic-candidates">
          <summary>Interprétations alternatives</summary>
          <ul>
            {field.candidates.map((candidate, index) => (
              <li key={`${candidate.displayValue}-${index}`}>
                <span>{friendlySemanticValue(field.fieldKey, candidate.displayValue)}</span>
                <small>{Math.round(candidate.confidence * 100)} %</small>
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      {field.evidence.length > 0 ? (
        <details className="semantic-why">
          <summary>Pourquoi ?</summary>
          {field.evidence.map((evidence, index) => (
            <div
              className="semantic-evidence"
              key={`${evidence.evidenceType}-${evidence.startOffset ?? index}`}
            >
              <q>{evidence.exactText}</q>
              <p>
                {evidence.explanation}
                {evidence.pageNumber ? ` · page ${evidence.pageNumber}` : ""}
                {evidence.sheetName ? ` · feuille ${evidence.sheetName}` : ""}
                {evidence.slideNumber ? ` · diapositive ${evidence.slideNumber}` : ""}
              </p>
              <small>
                {friendlyReason(evidence.extractionMethod)} · analyseur{" "}
                {evidence.analyzerVersion}
              </small>
            </div>
          ))}
        </details>
      ) : null}

      {editing ? (
        <div className="semantic-correction-editor">
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
          <div>
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
        <div className="semantic-field-actions">
          {field.displayValue && field.valueSource !== "USER" ? (
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
      <dd className={mono ? "mono-value" : undefined}>{value}</dd>
    </div>
  );
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

function friendlyInputQuality(value: string): string {
  const labels: Record<string, string> = {
    GOOD: "de bonne qualité",
    DEGRADED: "dégradée",
    POOR: "de faible qualité",
    UNUSABLE: "insuffisante",
  };
  return labels[value.toUpperCase()] ?? friendlyReason(value);
}

function friendlyFieldKey(value: string): string {
  const labels: Record<string, string> = {
    DOCUMENT_TYPE: "Type de document",
    CONTEXT: "Contexte",
    SUPPLIER_CANDIDATE: "Fournisseur candidat",
    CUSTOMER_CANDIDATE: "Client candidat",
    ISSUER: "Émetteur",
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
    PROJECT_REFERENCE_CANDIDATE: "Projet ou référence candidat",
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

function confidenceTone(field: SemanticField): "high" | "medium" | "review" | "human" {
  if (field.valueSource === "USER") {
    return "human";
  }
  if (["AMBIGUOUS", "CONFLICTING", "UNKNOWN"].includes(field.status)) {
    return "review";
  }
  if (field.confidence >= 0.85) {
    return "high";
  }
  return field.confidence >= 0.65 ? "medium" : "review";
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
