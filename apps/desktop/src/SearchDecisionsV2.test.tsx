// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { ReviewView } from "./ReviewView";
import { SearchView } from "./SearchView";
import type { FileReviewItem } from "./types";

vi.mock("./api", () => ({
  searchLocalFiles: vi.fn(),
  getEmbeddingModelStatus: vi.fn(),
  activateLocalEmbeddingModel: vi.fn(),
  cancelLocalEmbeddingModelInstall: vi.fn(),
  retryLocalEmbeddingModel: vi.fn(),
  removeLocalEmbeddingModel: vi.fn(),
  listReviewItems: vi.fn(),
  updateReviewItem: vi.fn(),
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
}));

const REVIEW_ITEMS: FileReviewItem[] = Array.from({ length: 5 }, (_, index) => ({
  reviewId: `review-${index + 1}`,
  fileId: `file-${index + 1}`,
  filename: `scan-${index + 1}.pdf`,
  relativePath: `Scans/scan-${index + 1}.pdf`,
  reason: "OCR_PROVIDER_UNAVAILABLE",
  sourceSubsystem: "EXTRACTION",
  severity: "WARNING",
  explanation: "La reconnaissance locale est indisponible.",
  status: "NEEDS_REVIEW",
  retryAvailable: false,
  retryCount: 0,
  extractionStatus: "PARTIAL",
  createdAt: "2026-08-24T10:00:00Z",
  updatedAt: "2026-08-24T10:00:00Z",
}));

describe("Search and Decisions V2", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getEmbeddingModelStatus).mockResolvedValue({
      modelId: "granite-embedding-97m-multilingual-r2",
      version: "test",
      dimensions: 384,
      status: "ready",
      approximateDiskBytes: 123_549_550,
      license: "Apache-2.0",
      localOnly: true,
      downloadImplemented: true,
      lastError: null,
      installRoot: "/tmp/models",
    });
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
      query: "facture point p chantier martin",
      page: 0,
      pageSize: 30,
      total: 1,
      hasMore: false,
      interpretedQuery: [],
      embeddings: {
        availability: "available_production",
        providerId: "granite-embedding-97m-multilingual-r2",
        version: "test",
        productionReady: true,
        indexedFiles: 100,
      },
      timings: {
        totalMs: 5,
        lexicalAndStructuredMs: 2,
        queryEmbedMs: 1,
        annMs: 1,
        vectorMs: 0,
        fusionMs: 1,
      },
      results: [
        {
          fileId: "file-search",
          filename: "00482.pdf",
          relativePath: "Clients/Martin/Factures/00482.pdf",
          extension: "pdf",
          detectedType: "application/pdf",
          byteSize: 2048,
          modifiedAt: "2026-08-20T12:00:00Z",
          extractionStatus: "success",
          ocrStatus: "not_used",
          duplicate: false,
          matchSource: "semantic",
          relevance: 0.94,
          snippet: "Facture Point P pour le chantier Martin",
          whyMatched: ["sens et contexte correspondants"],
        },
      ],
    });
    vi.mocked(api.listReviewItems).mockResolvedValue({
      total: REVIEW_ITEMS.length,
      limit: 500,
      offset: 0,
      hasMore: false,
      items: REVIEW_ITEMS,
    });
    vi.mocked(api.updateReviewItem).mockResolvedValue({
      ...REVIEW_ITEMS[0],
      status: "RESOLVED",
    });
  });

  it("shows a folder-by-folder path for semantic search results", async () => {
    render(
      <SearchView
        workspaceId="workspace-1"
        initialQuery="facture point p chantier martin"
        onOpenFile={vi.fn()}
      />,
    );

    expect(await screen.findByText("Clients")).toBeTruthy();
    expect(screen.getByText("Martin")).toBeTruthy();
    expect(screen.getAllByText("Factures").length).toBeGreaterThan(0);
    expect(screen.getAllByText("00482.pdf").length).toBeGreaterThan(0);
    expect(screen.getByText(/Recherche hybride/)).toBeTruthy();
    expect(api.searchLocalFiles).toHaveBeenCalledWith(
      "workspace-1",
      expect.objectContaining({ semanticSearch: true }),
    );
  });

  it("turns many identical review signals into one decision", async () => {
    render(<ReviewView workspaceId="workspace-1" onOpenFile={vi.fn()} />);

    expect(await screen.findByText("1")).toBeTruthy();
    expect(screen.getByText("décision")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Régler les 5 fichiers" })).toBeTruthy();
    expect(screen.getByText("+ 2 autres")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Régler les 5 fichiers" }));
    await waitFor(() => {
      expect(api.updateReviewItem).toHaveBeenCalledTimes(5);
    });
    for (const item of REVIEW_ITEMS) {
      expect(api.updateReviewItem).toHaveBeenCalledWith(item.reviewId, "resolve");
    }
  });
});
