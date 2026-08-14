// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { FileDetailPanel } from "./FileDetailPanel";
import type { LocalFileDetail } from "./types";

vi.mock("./api", () => ({
  getFileDetail: vi.fn(),
  storeSemanticCorrection: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
}));

const machineDetail: LocalFileDetail = {
  fileId: "file-1",
  fileVersionId: "version-1",
  filename: "scan_38492.pdf",
  relativePath: "Invoices/scan_38492.pdf",
  extension: "pdf",
  detectedType: "application/pdf",
  byteSize: 1_024,
  duplicate: false,
  extractionStatus: "SUCCESS",
  extractorType: "pdf_text",
  extractorVersion: "0.1.0",
  ocrStatus: "NOT_USED",
  textPreview: "POINT P\nFacture FP-39482",
  characterCount: 48,
  reviewItems: [],
  relationships: [],
  semanticAnalysis: {
    analysisId: "analysis-1",
    status: "SUCCESS",
    analyzerId: "deterministic-document-understanding",
    analyzerVersion: "5.0.0",
    providerId: "builtin-local-rules",
    providerVersion: "5.0.0",
    schemaVersion: 1,
    inputQuality: 1,
    inputQualityStatus: "GOOD",
    inputQualityReasons: [],
    language: "fr",
    analyzedAt: "2026-08-10T20:00:00Z",
    fields: [
      {
        fieldId: "field-type",
        fieldKey: "DOCUMENT_TYPE",
        valueKind: "DOCUMENT_TYPE",
        displayValue: "invoice",
        machineDisplayValue: "invoice",
        normalizedValue: { kind: "document_type", value: "invoice" },
        confidence: 0.98,
        status: "CONFIRMED",
        sourceMethod: "DETERMINISTIC_RULE",
        analyzerVersion: "5.0.0",
        valueSource: "MACHINE",
        evidence: [
          {
            evidenceType: "TEXT_SPAN",
            exactText: "<script>deleteFiles()</script> FACTURE N° FP-39482",
            startOffset: 8,
            endOffset: 57,
            pageNumber: 1,
            sourceLabel: "scan_38492.pdf",
            explanation: "explicit invoice-number label",
            extractionMethod: "PDF_TEXT",
            analyzerVersion: "5.0.0",
          },
        ],
        candidates: [],
      },
      {
        fieldId: "field-context",
        fieldKey: "CONTEXT",
        valueKind: "CONTEXT",
        displayValue: "business",
        machineDisplayValue: "business",
        normalizedValue: { kind: "context", value: "business" },
        confidence: 0.92,
        status: "CONFIRMED",
        sourceMethod: "DETERMINISTIC_RULE",
        analyzerVersion: "5.0.0",
        valueSource: "MACHINE",
        evidence: [],
        candidates: [],
      },
    ],
    entities: [
      {
        entityId: "entity-1",
        candidateKey: "supplier_candidate:point p",
        entityType: "SUPPLIER_CANDIDATE",
        originalValue: "POINT P",
        normalizedValue: "POINT P",
        confidence: 0.91,
        status: "CONFIRMED",
        sourceMethod: "DETERMINISTIC_RULE",
        evidence: [],
      },
    ],
  },
};

describe("Milestone 5 file understanding", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getFileDetail).mockResolvedValue(machineDetail);
    vi.mocked(api.storeSemanticCorrection).mockResolvedValue({
      correctionId: "correction-1",
      fileId: "file-1",
      fieldKey: "DOCUMENT_TYPE",
      correctionState: "USER_CORRECTED",
      valueKind: "DOCUMENT_TYPE",
      displayValue: "quote",
      normalizedValue: { kind: "document_type", value: "quote" },
      createdAt: "2026-08-10T20:01:00Z",
      updatedAt: "2026-08-10T20:01:00Z",
    });
  });

  it("shows confidence and provenance without rendering untrusted evidence as HTML", async () => {
    const { container } = render(
      <FileDetailPanel fileId="file-1" onClose={() => undefined} />,
    );

    expect(await screen.findByText("Facture")).toBeTruthy();
    expect(screen.getByText("Très fiable")).toBeTruthy();
    fireEvent.click(screen.getByText("Pourquoi ?"));
    expect(
      screen.getByText("<script>deleteFiles()</script> FACTURE N° FP-39482"),
    ).toBeTruthy();
    expect(screen.getByText(/page 1/)).toBeTruthy();
    expect(container.querySelector("script")).toBeNull();
  });

  it("stores user corrections separately and refreshes the effective value", async () => {
    const corrected: LocalFileDetail = {
      ...machineDetail,
      semanticAnalysis: {
        ...machineDetail.semanticAnalysis!,
        fields: machineDetail.semanticAnalysis!.fields.map((field) =>
          field.fieldKey === "DOCUMENT_TYPE"
            ? {
                ...field,
                displayValue: "quote",
                machineDisplayValue: "invoice",
                valueSource: "USER",
                userState: "USER_CORRECTED",
              }
            : field,
        ),
      },
    };
    vi.mocked(api.getFileDetail)
      .mockResolvedValueOnce(machineDetail)
      .mockResolvedValueOnce(corrected);

    render(<FileDetailPanel fileId="file-1" onClose={() => undefined} />);
    expect(await screen.findByText("Facture")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Corriger" })[0]);
    fireEvent.change(screen.getByLabelText("Correction"), {
      target: { value: "quote" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Enregistrer la correction" }),
    );

    await waitFor(() => {
      expect(api.storeSemanticCorrection).toHaveBeenCalledWith(
        "file-1",
        "document_type",
        "correct",
        "quote",
      );
    });
    expect(await screen.findByText("Devis")).toBeTruthy();
    expect(screen.getByText("Corrigé par vous")).toBeTruthy();
    expect(screen.getByText(/Valeur machine/)).toBeTruthy();
  });
});
