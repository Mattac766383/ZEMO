import { describe, expect, it } from "vitest";
import type { IdentityRelationship, LocalFileDetail, LocalSearchResult, SemanticField } from "./types";
import { buildKnowledgeMapModel } from "./knowledgeMapModel";

function result(fileId: string, filename: string): LocalSearchResult {
  return {
    fileId,
    filename,
    relativePath: `Documents/${filename}`,
    detectedType: "pdf",
    extension: "pdf",
    byteSize: 1024,
    modifiedAt: "2026-04-12T12:00:00Z",
    extractionStatus: "success",
    ocrStatus: "not_used",
    duplicate: false,
    matchSource: "metadata",
    relevance: 1,
    snippet: "",
    whyMatched: [],
  };
}

function field(key: string, value: string, valueSource = "MACHINE"): SemanticField {
  return {
    fieldId: `field-${key}-${value}`,
    fieldKey: key,
    valueKind: "text",
    displayValue: value,
    machineDisplayValue: null,
    normalizedValue: value,
    confidence: valueSource === "USER" ? 1 : 0.92,
    status: valueSource === "USER" ? "CONFIRMED" : "INFERRED",
    sourceMethod: "test",
    analyzerVersion: "test-1",
    valueSource,
    userState: valueSource === "USER" ? "USER_CORRECTED" : null,
    evidence: [],
    candidates: [],
  };
}

function relation(identityId: string, relationshipId: string): IdentityRelationship {
  return {
    relationshipId,
    relationshipType: "FILE_SUPPLIER",
    identityId,
    displayName: "Point P",
    identityType: "ORGANIZATION",
    confidence: 0.96,
    status: "CONFIRMED",
    userConfirmationState: null,
    evidence: ["preuve locale"],
  };
}

function detail(
  fileId: string,
  filename: string,
  fields: SemanticField[] = [],
  relationships: IdentityRelationship[] = [],
): LocalFileDetail {
  return {
    fileId,
    fileVersionId: `version-${fileId}`,
    filename,
    relativePath: `Documents/${filename}`,
    extension: "pdf",
    detectedType: "pdf",
    byteSize: 1024,
    createdAt: "2026-04-12T11:00:00Z",
    modifiedAt: "2026-04-12T12:00:00Z",
    hash: null,
    duplicate: false,
    extractionStatus: "SUCCESS",
    extractorType: "pdf",
    extractorVersion: "1",
    ocrStatus: "NOT_USED",
    textPreview: "",
    characterCount: 120,
    reviewItems: [],
    semanticAnalysis: {
      analysisId: `analysis-${fileId}`,
      status: "COMPLETED",
      analyzerId: "local",
      analyzerVersion: "test-1",
      providerId: "deterministic",
      providerVersion: "1",
      schemaVersion: 1,
      inputQuality: 1,
      inputQualityStatus: "GOOD",
      inputQualityReasons: [],
      language: "fr",
      analyzedAt: "2026-04-12T12:01:00Z",
      fields,
      entities: [],
    },
    relationships,
  };
}

describe("buildKnowledgeMapModel", () => {
  it("deduplicates resolved identities by identity id", () => {
    const model = buildKnowledgeMapModel([
      {
        result: result("file-a", "a.pdf"),
        detail: detail("file-a", "a.pdf", [field("context", "business")], [relation("point-p", "rel-a")]),
      },
      {
        result: result("file-b", "b.pdf"),
        detail: detail("file-b", "b.pdf", [field("context", "business")], [relation("point-p", "rel-b")]),
      },
    ]);
    const nodes = model.nodes.filter((node) => node.id === "identity:point-p");
    expect(nodes).toHaveLength(1);
    expect(nodes[0].fileCount).toBe(2);
  });

  it("does not invent business identities without evidence", () => {
    const model = buildKnowledgeMapModel([
      { result: result("file-a", "unknown.pdf"), detail: detail("file-a", "unknown.pdf") },
    ]);
    expect(model.nodes.filter((node) => ["project", "person", "organization"].includes(node.kind))).toEqual([]);
  });

  it("uses effective user-corrected semantic values", () => {
    const corrected = field("context", "business", "USER");
    corrected.machineDisplayValue = "personal";
    const model = buildKnowledgeMapModel([
      { result: result("file-a", "corrected.pdf"), detail: detail("file-a", "corrected.pdf", [corrected]) },
    ]);
    expect(model.nodes.find((node) => node.id === "context:business")?.fileCount).toBe(1);
    expect(model.nodes.find((node) => node.id === "context:personal")).toBeUndefined();
  });

  it("is deterministic regardless of input order", () => {
    const a = {
      result: result("file-a", "a.pdf"),
      detail: detail("file-a", "a.pdf", [field("context", "business"), field("document_type", "invoice")]),
    };
    const b = {
      result: result("file-b", "b.pdf"),
      detail: detail("file-b", "b.pdf", [field("context", "personal"), field("document_type", "insurance_document")]),
    };
    const left = buildKnowledgeMapModel([a, b]);
    const right = buildKnowledgeMapModel([b, a]);
    expect(left.nodes).toEqual(right.nodes);
    expect(left.edges).toEqual(right.edges);
  });
});
