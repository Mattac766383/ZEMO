import type {
  IdentityRelationship,
  LocalFileDetail,
  LocalSearchResult,
  SemanticField,
} from "./types";

export type KnowledgeContext = "business" | "personal" | "mixed" | "unknown";
export type KnowledgeNodeKind =
  | "root"
  | "context"
  | "organization"
  | "person"
  | "project"
  | "document_type"
  | "year";

export interface KnowledgeMapFile {
  fileId: string;
  filename: string;
  relativePath: string;
  detectedType?: string | null;
  extension?: string | null;
  byteSize: number;
  modifiedAt?: string | null;
  needsReview: boolean;
}

export interface KnowledgeMapNode {
  id: string;
  kind: KnowledgeNodeKind;
  label: string;
  fileCount: number;
  reviewCount: number;
  confidence?: number | null;
  identityId?: string | null;
  relationshipKind?: string | null;
  contexts: KnowledgeContext[];
  fileIds: string[];
}

export interface KnowledgeMapEdge {
  id: string;
  sourceId: string;
  targetId: string;
  kind: string;
  confidence?: number | null;
}

export interface KnowledgeMapModel {
  nodes: KnowledgeMapNode[];
  edges: KnowledgeMapEdge[];
  files: Record<string, KnowledgeMapFile>;
  detailedFiles: number;
  needsReview: number;
}

export interface KnowledgeMapInput {
  result: LocalSearchResult;
  detail: LocalFileDetail | null;
}

type MutableNode = Omit<KnowledgeMapNode, "fileIds" | "contexts"> & {
  fileIds: Set<string>;
  contexts: Set<KnowledgeContext>;
};

const DOCUMENT_TYPE_LABELS: Record<string, string> = {
  invoice: "Factures",
  quote: "Devis",
  contract: "Contrats",
  purchase_order: "Bons de commande",
  delivery_note: "Bons de livraison",
  bank_statement: "Relevés bancaires",
  tax_document: "Documents fiscaux",
  payslip: "Bulletins de paie",
  employment_contract: "Contrats de travail",
  insurance_document: "Assurances",
  legal_document: "Documents juridiques",
  administrative_document: "Administratif",
  receipt: "Reçus",
  report: "Rapports",
  letter: "Lettres",
  cv: "CV",
  photo: "Photos",
  video: "Vidéos",
  spreadsheet: "Tableurs",
  presentation: "Présentations",
  archive: "Archives",
  other: "Autres documents",
  unknown: "Type à déterminer",
};

export function buildKnowledgeMapModel(inputs: KnowledgeMapInput[]): KnowledgeMapModel {
  const rows = [...inputs].sort((left, right) =>
    left.result.filename.localeCompare(right.result.filename, "fr", { sensitivity: "base" }),
  );
  const nodes = new Map<string, MutableNode>();
  const edges = new Map<string, KnowledgeMapEdge>();
  const files: Record<string, KnowledgeMapFile> = {};
  let detailedFiles = 0;
  let totalReview = 0;

  ensureNode(nodes, {
    id: "root:computer",
    kind: "root",
    label: "Mon ordinateur",
  });

  for (const { result, detail } of rows) {
    const fileId = result.fileId;
    const needsReview =
      detail?.reviewItems.some((item) => item.status.toUpperCase() === "NEEDS_REVIEW") ?? false;
    if (detail) {
      detailedFiles += 1;
    }
    if (needsReview) {
      totalReview += 1;
    }
    files[fileId] = {
      fileId,
      filename: result.filename,
      relativePath: result.relativePath,
      detectedType: result.detectedType,
      extension: result.extension,
      byteSize: result.byteSize,
      modifiedAt: result.modifiedAt,
      needsReview,
    };

    const context = contextFor(detail);
    const contextNode = contextDescriptor(context);
    const contextId = `context:${context}`;
    addMembership(nodes, "root:computer", fileId, context, needsReview);
    ensureNode(nodes, { id: contextId, kind: "context", label: contextNode });
    addMembership(nodes, contextId, fileId, context, needsReview);
    ensureEdge(edges, "root:computer", contextId, "contains");

    const documentType = semanticValue(detail, ["document_type"]);
    const detectedFallback = normalizeDetectedType(result.detectedType, result.extension);
    const documentKey = documentType?.toLowerCase() ?? detectedFallback.key;
    const documentLabel = documentType
      ? (DOCUMENT_TYPE_LABELS[documentKey] ?? humanize(documentType))
      : detectedFallback.label;
    if (documentLabel) {
      const nodeId = `document-type:${normalizeKey(documentKey)}`;
      ensureNode(nodes, { id: nodeId, kind: "document_type", label: documentLabel });
      addMembership(nodes, nodeId, fileId, context, needsReview);
      ensureEdge(edges, contextId, nodeId, "document_type");
    }

    const year = yearFor(detail, result.modifiedAt);
    if (year) {
      const nodeId = `year:${year}`;
      ensureNode(nodes, { id: nodeId, kind: "year", label: String(year) });
      addMembership(nodes, nodeId, fileId, context, needsReview);
      ensureEdge(edges, contextId, nodeId, "year");
    }

    const relationships = detail?.relationships ?? [];
    for (const relationship of relationships) {
      const node = relationshipDescriptor(relationship);
      const nodeId = `identity:${relationship.identityId}`;
      ensureNode(nodes, {
        id: nodeId,
        kind: node.kind,
        label: relationship.displayName,
        identityId: relationship.identityId,
        relationshipKind: relationship.relationshipType,
        confidence: relationship.confidence,
      });
      addMembership(nodes, nodeId, fileId, context, needsReview, relationship.confidence);
      ensureEdge(
        edges,
        contextId,
        nodeId,
        relationship.relationshipType.toLowerCase(),
        relationship.confidence,
      );
    }

    addSemanticFallback(
      nodes,
      edges,
      detail,
      contextId,
      fileId,
      context,
      needsReview,
      "project",
      ["project", "project_reference_candidate"],
    );
    addSemanticFallback(
      nodes,
      edges,
      detail,
      contextId,
      fileId,
      context,
      needsReview,
      "organization",
      ["supplier", "supplier_candidate", "issuer"],
      "supplier",
    );
    addSemanticFallback(
      nodes,
      edges,
      detail,
      contextId,
      fileId,
      context,
      needsReview,
      "organization",
      ["customer", "customer_candidate"],
      "customer",
    );
  }

  const finalizedNodes = [...nodes.values()]
    .map((node): KnowledgeMapNode => ({
      ...node,
      contexts: [...node.contexts].sort(),
      fileIds: [...node.fileIds].sort(),
      fileCount: node.fileIds.size,
    }))
    .sort(compareNodes);

  return {
    nodes: finalizedNodes,
    edges: [...edges.values()].sort((left, right) => left.id.localeCompare(right.id)),
    files,
    detailedFiles,
    needsReview: totalReview,
  };
}

function ensureNode(
  nodes: Map<string, MutableNode>,
  input: {
    id: string;
    kind: KnowledgeNodeKind;
    label: string;
    identityId?: string | null;
    relationshipKind?: string | null;
    confidence?: number | null;
  },
): MutableNode {
  const existing = nodes.get(input.id);
  if (existing) {
    if ((input.confidence ?? 0) > (existing.confidence ?? 0)) {
      existing.confidence = input.confidence;
    }
    return existing;
  }
  const node: MutableNode = {
    ...input,
    fileCount: 0,
    reviewCount: 0,
    fileIds: new Set<string>(),
    contexts: new Set<KnowledgeContext>(),
  };
  nodes.set(input.id, node);
  return node;
}

function addMembership(
  nodes: Map<string, MutableNode>,
  nodeId: string,
  fileId: string,
  context: KnowledgeContext,
  needsReview: boolean,
  confidence?: number | null,
) {
  const node = nodes.get(nodeId);
  if (!node) {
    return;
  }
  const wasPresent = node.fileIds.has(fileId);
  node.fileIds.add(fileId);
  node.contexts.add(context);
  if (!wasPresent && needsReview) {
    node.reviewCount += 1;
  }
  if ((confidence ?? 0) > (node.confidence ?? 0)) {
    node.confidence = confidence;
  }
}

function ensureEdge(
  edges: Map<string, KnowledgeMapEdge>,
  sourceId: string,
  targetId: string,
  kind: string,
  confidence?: number | null,
) {
  const id = `${sourceId}->${targetId}:${kind}`;
  const existing = edges.get(id);
  if (existing) {
    if ((confidence ?? 0) > (existing.confidence ?? 0)) {
      existing.confidence = confidence;
    }
    return;
  }
  edges.set(id, { id, sourceId, targetId, kind, confidence });
}

function addSemanticFallback(
  nodes: Map<string, MutableNode>,
  edges: Map<string, KnowledgeMapEdge>,
  detail: LocalFileDetail | null,
  contextId: string,
  fileId: string,
  context: KnowledgeContext,
  needsReview: boolean,
  kind: "organization" | "project",
  keys: string[],
  relationshipKind = kind,
) {
  const value = semanticValue(detail, keys);
  if (!value) {
    return;
  }
  const alreadyResolved = detail?.relationships.some((relationship) =>
    relationship.displayName.localeCompare(value, "fr", { sensitivity: "base" }) === 0,
  );
  if (alreadyResolved) {
    return;
  }
  const nodeId = `semantic:${kind}:${normalizeKey(value)}`;
  ensureNode(nodes, {
    id: nodeId,
    kind,
    label: value,
    relationshipKind,
    confidence: semanticConfidence(detail, keys),
  });
  addMembership(
    nodes,
    nodeId,
    fileId,
    context,
    needsReview,
    semanticConfidence(detail, keys),
  );
  ensureEdge(
    edges,
    contextId,
    nodeId,
    relationshipKind,
    semanticConfidence(detail, keys),
  );
}

function semanticField(detail: LocalFileDetail | null, keys: string[]): SemanticField | null {
  if (!detail?.semanticAnalysis) {
    return null;
  }
  const normalized = new Set(keys.map((key) => key.toLowerCase()));
  return (
    detail.semanticAnalysis.fields.find(
      (field) => normalized.has(field.fieldKey.toLowerCase()) && Boolean(field.displayValue?.trim()),
    ) ?? null
  );
}

function semanticValue(detail: LocalFileDetail | null, keys: string[]): string | null {
  return semanticField(detail, keys)?.displayValue?.trim() || null;
}

function semanticConfidence(detail: LocalFileDetail | null, keys: string[]): number | null {
  return semanticField(detail, keys)?.confidence ?? null;
}

function contextFor(detail: LocalFileDetail | null): KnowledgeContext {
  const value = semanticValue(detail, ["context"])?.toLowerCase();
  if (value === "business" || value === "professional" || value === "professionnel") {
    return "business";
  }
  if (value === "personal" || value === "personnel") {
    return "personal";
  }
  if (value === "mixed" || value === "mixte") {
    return "mixed";
  }
  return "unknown";
}

function contextDescriptor(context: KnowledgeContext): string {
  switch (context) {
    case "business":
      return "Professionnel";
    case "personal":
      return "Personnel";
    case "mixed":
      return "Mixte";
    default:
      return "À déterminer";
  }
}

function yearFor(detail: LocalFileDetail | null, modifiedAt?: string | null): number | null {
  const direct = semanticValue(detail, ["year"]);
  const directYear = direct ? Number.parseInt(direct, 10) : Number.NaN;
  if (Number.isInteger(directYear) && directYear >= 1900 && directYear <= 2200) {
    return directYear;
  }
  const dateValue = semanticValue(detail, [
    "document_date",
    "issue_date",
    "date",
    "invoice_date",
  ]);
  const semanticYear = dateValue?.match(/(?:19|20|21)\d{2}/)?.[0];
  if (semanticYear) {
    return Number(semanticYear);
  }
  if (!modifiedAt) {
    return null;
  }
  try {
    const date = /^\d+$/.test(modifiedAt)
      ? new Date(Number(BigInt(modifiedAt) / 1_000_000n))
      : new Date(modifiedAt);
    const year = date.getFullYear();
    return Number.isFinite(year) && year >= 1900 && year <= 2200 ? year : null;
  } catch {
    return null;
  }
}

function relationshipDescriptor(relationship: IdentityRelationship): {
  kind: "organization" | "person" | "project";
} {
  const value = relationship.identityType.toLowerCase();
  if (value === "person") {
    return { kind: "person" };
  }
  if (value === "project") {
    return { kind: "project" };
  }
  return { kind: "organization" };
}

function normalizeDetectedType(
  detectedType?: string | null,
  extension?: string | null,
): { key: string; label: string } {
  const raw = detectedType?.trim() || extension?.replace(/^\./, "").trim() || "unknown";
  const lower = raw.toLowerCase();
  const common: Record<string, string> = {
    pdf: "PDF",
    image: "Images",
    video: "Vidéos",
    audio: "Audio",
    document: "Documents",
    spreadsheet: "Tableurs",
    presentation: "Présentations",
    archive: "Archives",
    unknown: "Type à déterminer",
  };
  return { key: lower, label: common[lower] ?? humanize(raw) };
}

function humanize(value: string): string {
  const normalized = value.replace(/[_-]+/g, " ").trim();
  return normalized.length > 0
    ? normalized.charAt(0).toLocaleUpperCase() + normalized.slice(1)
    : "À déterminer";
}

function normalizeKey(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 96) || "unknown";
}

function compareNodes(left: KnowledgeMapNode, right: KnowledgeMapNode): number {
  const order: Record<KnowledgeNodeKind, number> = {
    root: 0,
    context: 1,
    project: 2,
    organization: 3,
    person: 4,
    document_type: 5,
    year: 6,
  };
  return (
    order[left.kind] - order[right.kind] ||
    left.label.localeCompare(right.label, "fr", { sensitivity: "base" }) ||
    left.id.localeCompare(right.id)
  );
}
