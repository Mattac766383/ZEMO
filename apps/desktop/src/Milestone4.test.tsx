// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { ReviewView } from "./ReviewView";
import { SearchView } from "./SearchView";

vi.mock("./api", () => ({
  searchLocalFiles: vi.fn(),
  listReviewItems: vi.fn(),
  updateReviewItem: vi.fn(),
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
  getEmbeddingModelStatus: vi.fn().mockResolvedValue({
    modelId: "granite-embedding-97m-multilingual-r2",
    version: "test",
    dimensions: 384,
    status: "not_installed",
    approximateDiskBytes: 1,
    license: "Apache-2.0",
    localOnly: true,
    downloadImplemented: true,
    lastError: null,
    installRoot: "/tmp/models",
  }),
  activateLocalEmbeddingModel: vi.fn(),
  cancelLocalEmbeddingModelInstall: vi.fn(),
  retryLocalEmbeddingModel: vi.fn(),
  removeLocalEmbeddingModel: vi.fn(),
  rebuildSemanticAnnIndex: vi.fn(),
}));

describe("Milestone 4 product surfaces", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
      query: "facture",
      page: 0,
      pageSize: 50,
      total: 1,
      hasMore: false,
      interpretedQuery: [],
      embeddings: {
        availability: "unavailable",
        providerId: "unavailable-local-embedding",
        version: "none",
        productionReady: false,
        indexedFiles: 0,
      },
      timings: {
        totalMs: 1,
        lexicalAndStructuredMs: 1,
        queryEmbedMs: 0,
        annMs: 0,
        vectorMs: 0,
        fusionMs: 0,
      },
      results: [
        {
          fileId: "file-1",
          filename: "<img src=x onerror=alert(1)>.txt",
          relativePath: "Clients/<script>unsafe</script>.txt",
          extension: "txt",
          detectedType: "text/plain",
          byteSize: 42,
          modifiedAt: "2026-08-10T10:00:00Z",
          extractionStatus: "success",
          ocrStatus: "not_used",
          duplicate: false,
          matchSource: "content",
          relevance: 1,
          snippet: "<img src=x onerror=alert(1)> facture locale",
          whyMatched: ["texte du document correspondant"],
        },
      ],
    });
    vi.mocked(api.listReviewItems).mockResolvedValue({
      total: 1,
      limit: 50,
      offset: 0,
      hasMore: false,
      items: [
        {
          reviewId: "review-1",
          fileId: "file-1",
          filename: "scan.pdf",
          relativePath: "Scans/scan.pdf",
          reason: "OCR_PROVIDER_UNAVAILABLE",
          sourceSubsystem: "EXTRACTION",
          severity: "WARNING",
          explanation: "La reconnaissance locale est indisponible.",
          status: "NEEDS_REVIEW",
          retryAvailable: true,
          retryCount: 0,
          extractionStatus: "PARTIAL",
          createdAt: "2026-08-10T10:00:00Z",
          updatedAt: "2026-08-10T10:00:00Z",
        },
      ],
    });
    vi.mocked(api.updateReviewItem).mockResolvedValue({
      reviewId: "review-1",
      fileId: "file-1",
      filename: "scan.pdf",
      relativePath: "Scans/scan.pdf",
      reason: "OCR_PROVIDER_UNAVAILABLE",
      sourceSubsystem: "EXTRACTION",
      severity: "WARNING",
      explanation: "La reconnaissance locale est indisponible.",
      status: "RESOLVED",
      retryAvailable: true,
      retryCount: 0,
      extractionStatus: "PARTIAL",
      createdAt: "2026-08-10T10:00:00Z",
      updatedAt: "2026-08-10T10:01:00Z",
    });
    vi.mocked(api.retryExtraction).mockResolvedValue({
      reviewId: "review-1",
      fileId: "file-1",
      batchId: "batch-1",
      status: "UNAVAILABLE",
      extractionStatus: "PARTIAL",
      message: "La dépendance locale reste indisponible.",
    });
  });

  it("renders all untrusted search text as plain text", async () => {
    render(<SearchView workspaceId="workspace-1" onOpenFile={vi.fn()} />);

    expect(
      await screen.findByText("<img src=x onerror=alert(1)> facture locale"),
    ).toBeTruthy();
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("script")).toBeNull();

    const input = screen.getByRole("searchbox");
    fireEvent.change(input, { target: { value: `" OR () 🧾 facture` } });
    await waitFor(() => {
      expect(api.searchLocalFiles).toHaveBeenLastCalledWith(
        "workspace-1",
        expect.objectContaining({ text: `" OR () 🧾 facture` }),
      );
    });
  });

  it("supports review retry, resolve, ignore and detail actions", async () => {
    const openFile = vi.fn();
    render(<ReviewView workspaceId="workspace-1" onOpenFile={openFile} />);

    expect(await screen.findByText("La reconnaissance locale est indisponible.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Voir les détails" }));
    expect(openFile).toHaveBeenCalledWith("file-1");

    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    await waitFor(() => expect(api.retryExtraction).toHaveBeenCalledWith("review-1"));

    fireEvent.click(screen.getByRole("button", { name: "Résoudre" }));
    await waitFor(() =>
      expect(api.updateReviewItem).toHaveBeenCalledWith("review-1", "resolve"),
    );

    fireEvent.click(screen.getByRole("button", { name: "Ignorer" }));
    await waitFor(() =>
      expect(api.updateReviewItem).toHaveBeenCalledWith("review-1", "ignore"),
    );
  });
});
