// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { SearchView } from "./SearchView";

vi.mock("./api", () => ({
  searchLocalFiles: vi.fn(),
  getEmbeddingModelStatus: vi.fn(),
  activateLocalEmbeddingModel: vi.fn(),
  cancelLocalEmbeddingModelInstall: vi.fn(),
  retryLocalEmbeddingModel: vi.fn(),
  removeLocalEmbeddingModel: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
}));

describe("Milestone 9 hybrid search surface", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getEmbeddingModelStatus).mockResolvedValue({
      modelId: "granite-embedding-97m-multilingual-r2",
      version: "835ad14087e140460703cf0fae09f97d469d65c2",
      dimensions: 384,
      status: "not_installed",
      approximateDiskBytes: 123_549_550,
      license: "Apache-2.0",
      localOnly: true,
      downloadImplemented: true,
      lastError: null,
      installRoot: "/tmp/models",
    });
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
      query: "facture Point P environ 1400 euros chantier Martin",
      page: 0,
      pageSize: 50,
      total: 1,
      hasMore: false,
      interpretedQuery: [
        { id: "document_type", kind: "document_type", label: "Facture", value: "invoice" },
        { id: "party", kind: "party", label: "Point P", value: "point p" },
        { id: "amount", kind: "amount", label: "~1400€", value: "140000" },
        { id: "project", kind: "project", label: "Projet Martin", value: "martin" },
      ],
      embeddings: {
        availability: "unavailable",
        providerId: "granite-embedding-97m-multilingual-r2",
        version: "835ad14087e140460703cf0fae09f97d469d65c2",
        productionReady: false,
        indexedFiles: 0,
      },
      timings: {
        totalMs: 12,
        lexicalAndStructuredMs: 4,
        queryEmbedMs: 0,
        annMs: 0,
        vectorMs: 5,
        fusionMs: 3,
      },
      results: [
        {
          fileId: "file-1",
          filename: "00482.pdf",
          relativePath: "Scans/00482.pdf",
          extension: "pdf",
          detectedType: "application/pdf",
          byteSize: 1024,
          modifiedAt: "2026-06-17T10:00:00Z",
          extractionStatus: "success",
          ocrStatus: "not_used",
          duplicate: false,
          matchSource: "relationship",
          relevance: 0.92,
          snippet: "Facture chantier Martin",
          whyMatched: [
            "type de document correspondant : Facture",
            "fournisseur correspondant : Point P (confirmé)",
            "montant correspondant (1400 €)",
            "projet correspondant : Martin",
          ],
        },
      ],
    });
  });

  it("shows removable interpreted chips and concise explanations", async () => {
    render(<SearchView workspaceId="workspace-1" onOpenFile={vi.fn()} />);

    expect(await screen.findByText("Pourquoi ce résultat ?")).toBeTruthy();
    expect(screen.getByText("fournisseur correspondant : Point P (confirmé)")).toBeTruthy();
    expect(screen.getAllByText(/Recherche classique/i).length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole("button", { name: "Retirer Facture" }));
    await waitFor(() => {
      expect(api.searchLocalFiles).toHaveBeenLastCalledWith(
        "workspace-1",
        expect.objectContaining({
          semanticSearch: true,
          disabledIntents: ["document_type"],
        }),
      );
    });
  });

  it("exposes high-value structured filters without replacing the search bar", async () => {
    render(<SearchView workspaceId="workspace-1" onOpenFile={vi.fn()} />);
    await screen.findByRole("searchbox");

    fireEvent.click(screen.getByText("Filtres structurés"));
    fireEvent.change(screen.getByLabelText("Fournisseur"), {
      target: { value: "Point P" },
    });
    fireEvent.change(screen.getByLabelText("Année"), {
      target: { value: "2026" },
    });
    fireEvent.change(screen.getByLabelText("Montant min. (€)"), {
      target: { value: "1000" },
    });

    await waitFor(() => {
      expect(api.searchLocalFiles).toHaveBeenLastCalledWith(
        "workspace-1",
        expect.objectContaining({
          filters: expect.objectContaining({
            supplier: "Point P",
            year: 2026,
            amountMinimumMinor: 100_000,
          }),
        }),
      );
    });
    expect(screen.getAllByRole("searchbox")).toHaveLength(1);
  });

  it("shows local semantic model status and install controls", async () => {
    vi.mocked(api.activateLocalEmbeddingModel).mockResolvedValue({
      modelId: "granite-embedding-97m-multilingual-r2",
      version: "835ad14087e140460703cf0fae09f97d469d65c2",
      dimensions: 384,
      status: "ready",
      approximateDiskBytes: 123_549_550,
      license: "Apache-2.0",
      localOnly: true,
      downloadImplemented: true,
      lastError: null,
      installRoot: "/tmp/models",
    });

    render(<SearchView workspaceId="workspace-1" onOpenFile={vi.fn()} />);
    expect(await screen.findByText("Recherche intelligente")).toBeTruthy();
    expect(screen.getByText(/Recherche améliorée non activée/)).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Activer",
      }),
    );
    await waitFor(() => {
      expect(api.activateLocalEmbeddingModel).toHaveBeenCalled();
      expect(screen.getByText(/Recherche améliorée active/)).toBeTruthy();
    });
  });
});
