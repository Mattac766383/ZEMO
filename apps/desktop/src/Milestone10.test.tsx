// @vitest-environment jsdom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { MonitoringView } from "./MonitoringView";
import type { MonitoringDashboard } from "./types";

vi.mock("./api", () => ({
  addMonitoringExclusion: vi.fn(),
  cancelMonitoring: vi.fn(),
  getErrorMessage: (error: unknown) =>
    error instanceof Error ? `Safe error: ${error.message}` : `Safe error: ${String(error)}`,
  getMonitoringDashboard: vi.fn(),
  pauseMonitoring: vi.fn(),
  removeMonitoringExclusion: vi.fn(),
  resumeMonitoring: vi.fn(),
  runMonitoringCycle: vi.fn(),
  setMonitoringMode: vi.fn(),
  setMonitoredFolderEnabled: vi.fn(),
}));

const maliciousPath = '<script data-path="true">stealFiles()</script>';

const baseDashboard: MonitoringDashboard = {
  workspaceId: "workspace-10",
  mode: "PRUDENT",
  paused: false,
  startupReconciliationPending: false,
  automaticExecutionEnabled: false,
  folders: [
    {
      rootId: "root-inbox",
      displayLabel: "Personal inbox",
      selectedPath: maliciousPath,
      enabled: true,
      status: "WATCHING",
      pendingJobs: 1,
      lastReconciledAt: "2026-08-11T10:00:00Z",
    },
    {
      rootId: "root-archive",
      displayLabel: "Local archive",
      selectedPath: "/Users/example/Documents/Archive",
      enabled: false,
      status: "PAUSED",
      pendingJobs: 0,
      lastReconciledAt: null,
    },
  ],
  counts: {
    filesAnalyzed: 12,
    readyToOrganize: 2,
    needsReview: 1,
    pendingProposals: 4,
    pendingJobs: 1,
  },
  recentActivity: [
    {
      id: "activity-10",
      summary: "One local monitoring cycle completed",
      filesAnalyzed: 3,
      readyToOrganize: 2,
      needsReview: 1,
      failed: 0,
      createdAt: "2026-08-11T10:01:00Z",
    },
  ],
  exclusions: [
    {
      id: "exclusion-bak",
      rootId: "root-archive",
      kind: "extension",
      value: ".bak",
      enabled: true,
    },
  ],
};

function copyDashboard(
  overrides: Partial<MonitoringDashboard> = {},
): MonitoringDashboard {
  return {
    ...baseDashboard,
    ...overrides,
    folders: overrides.folders ?? baseDashboard.folders.map((folder) => ({ ...folder })),
    counts: { ...baseDashboard.counts, ...overrides.counts },
    recentActivity:
      overrides.recentActivity ??
      baseDashboard.recentActivity.map((activity) => ({ ...activity })),
    exclusions:
      overrides.exclusions ??
      baseDashboard.exclusions.map((exclusion) => ({ ...exclusion })),
  };
}

describe("Milestone 10 monitoring dashboard", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue(copyDashboard());
    vi.mocked(api.pauseMonitoring).mockResolvedValue(undefined);
    vi.mocked(api.resumeMonitoring).mockResolvedValue(undefined);
    vi.mocked(api.setMonitoredFolderEnabled).mockResolvedValue(undefined);
    vi.mocked(api.addMonitoringExclusion).mockResolvedValue(undefined);
    vi.mocked(api.removeMonitoringExclusion).mockResolvedValue(undefined);
    vi.mocked(api.runMonitoringCycle).mockResolvedValue(copyDashboard());
    vi.mocked(api.setMonitoringMode).mockResolvedValue(copyDashboard());
    vi.mocked(api.cancelMonitoring).mockResolvedValue(undefined);
  });

  it("keeps monitoring proposal-only and renders aggregate activity safely", async () => {
    const { container } = render(
      <MonitoringView workspaceId="workspace-10" />,
    );

    expect(
      screen.getByText("Surveillance = propositions uniquement"),
    ).toBeTruthy();
    expect(
      await screen.findByRole("heading", { name: "Vue d’ensemble" }),
    ).toBeTruthy();
    expect(
      screen.getByText(/La surveillance prépare des propositions/i),
    ).toBeTruthy();
    expect(screen.getByLabelText("Fichiers analysés").textContent).toContain("12");
    expect(screen.getByLabelText("Prêts à organiser").textContent).toContain("2");
    expect(screen.getByLabelText("À revoir").textContent).toContain("1");
    expect(screen.getByLabelText("Propositions en attente").textContent).toContain("4");
    expect(screen.getByLabelText("Tâches en attente").textContent).toContain("1");
    expect(
      screen.getByText(
        "3 nouveaux fichiers analysés · 2 prêts à organiser · 1 à revoir",
      ),
    ).toBeTruthy();
    expect(screen.getByText(maliciousPath)).toBeTruthy();
    expect(container.querySelector("script")).toBeNull();
    expect(
      screen.queryByRole("button", {
        name: /^(move|rename|delete|apply)\b/i,
      }),
    ).toBeNull();
  });

  it("renders healthy, paused, degraded, overflowed, offline, and error states distinctly", async () => {
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue(
      copyDashboard({
        folders: [
          {
            ...baseDashboard.folders[0],
            rootId: "healthy",
            displayLabel: "Healthy folder",
            status: "WATCHING",
          },
          {
            ...baseDashboard.folders[0],
            rootId: "degraded",
            displayLabel: "Degraded folder",
            status: "RECONCILING",
          },
          {
            ...baseDashboard.folders[0],
            rootId: "overflowed",
            displayLabel: "Overflowed folder",
            status: "OVERFLOWED",
          },
          {
            ...baseDashboard.folders[0],
            rootId: "offline",
            displayLabel: "Offline folder",
            status: "OFFLINE",
          },
          {
            ...baseDashboard.folders[0],
            rootId: "failed",
            displayLabel: "Failed folder",
            status: "FAILED",
          },
          {
            ...baseDashboard.folders[1],
            rootId: "paused",
            displayLabel: "Paused folder",
            status: "PAUSED",
          },
        ],
      }),
    );

    render(<MonitoringView workspaceId="workspace-10" />);
    expect(await screen.findByText("État : Saine")).toBeTruthy();
    expect(screen.getByText("État : Dégradée")).toBeTruthy();
    expect(screen.getByText("État : Saturée")).toBeTruthy();
    expect(screen.getByText("État : Hors ligne")).toBeTruthy();
    expect(screen.getByText("État : Erreur")).toBeTruthy();
    expect(screen.getByText("État : En pause")).toBeTruthy();
    expect(screen.getByLabelText("État de la surveillance").textContent).toContain(
      "Erreur",
    );
    for (const status of ["Overflowed", "Offline", "Failed"]) {
      expect(
        screen
          .getByLabelText(`État du dossier : ${status}`)
          .classList.contains("error"),
      ).toBe(true);
    }
    expect(
      screen
        .getByLabelText("État du dossier : Watching")
        .classList.contains("ready"),
    ).toBe(true);
  });


  it("enables automatic organization explicitly and explains the 92 percent gate", async () => {
    const automatic = copyDashboard({
      mode: "AUTOMATIC",
      automaticExecutionEnabled: true,
      counts: { pendingJobs: 0 },
    });
    vi.mocked(api.setMonitoringMode).mockResolvedValue(automatic);

    render(<MonitoringView workspaceId="workspace-10" />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Activer le rangement automatique",
      }),
    );

    await waitFor(() => {
      expect(api.setMonitoringMode).toHaveBeenCalledWith(
        "workspace-10",
        "AUTOMATIC",
      );
    });
    expect(await screen.findByText("Rangement automatique actif")).toBeTruthy();
    expect(screen.getByText(/92 % de confiance/i)).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Revenir au mode prudent" }),
    ).toBeTruthy();
  });

  it("pauses and resumes global monitoring", async () => {
    vi.mocked(api.getMonitoringDashboard)
      .mockResolvedValueOnce(copyDashboard())
      .mockResolvedValue(
        copyDashboard({
          paused: true,
        }),
      );

    render(<MonitoringView workspaceId="workspace-10" />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Mettre en pause" }),
    );
    await waitFor(() => {
      expect(api.pauseMonitoring).toHaveBeenCalledWith("workspace-10");
    });

    fireEvent.click(
      await screen.findByRole("button", { name: "Reprendre la surveillance" }),
    );
    await waitFor(() => {
      expect(api.resumeMonitoring).toHaveBeenCalledWith("workspace-10");
    });
  });

  it("enables and disables individual monitored folders", async () => {
    render(<MonitoringView workspaceId="workspace-10" />);

    fireEvent.click(
      await screen.findByRole("button", { name: "Désactiver Personal inbox" }),
    );
    await waitFor(() => {
      expect(api.setMonitoredFolderEnabled).toHaveBeenCalledWith(
        "root-inbox",
        false,
      );
    });

    const enableArchive = await screen.findByRole("button", {
      name: "Activer Local archive",
    });
    await waitFor(() => {
      expect((enableArchive as HTMLButtonElement).disabled).toBe(false);
    });
    fireEvent.click(enableArchive);
    await waitFor(() => {
      expect(api.setMonitoredFolderEnabled).toHaveBeenCalledWith(
        "root-archive",
        true,
      );
    });
  });

  it("sanitizes additions, rejects unsafe paths, and removes exclusions", async () => {
    render(<MonitoringView workspaceId="workspace-10" />);
    await screen.findByRole("heading", { name: "Exclusions" });

    fireEvent.change(screen.getByLabelText("Type d’exclusion"), {
      target: { value: "extension" },
    });
    fireEvent.change(screen.getByLabelText("Périmètre"), {
      target: { value: "root-inbox" },
    });
    fireEvent.change(screen.getByLabelText("Valeur"), {
      target: { value: "  .TMP  " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Ajouter une exclusion" }));
    await waitFor(() => {
      expect(api.addMonitoringExclusion).toHaveBeenCalledWith(
        "workspace-10",
        "root-inbox",
        "extension",
        ".tmp",
      );
    });

    const remove = screen.getByRole("button", {
      name: "Retirer l’exclusion .bak",
    });
    await waitFor(() => {
      expect((remove as HTMLButtonElement).disabled).toBe(false);
    });
    fireEvent.click(remove);
    await waitFor(() => {
      expect(api.removeMonitoringExclusion).toHaveBeenCalledWith(
        "exclusion-bak",
      );
    });

    await waitFor(() => {
      expect(
        (
          screen.getByRole("button", {
            name: "Ajouter une exclusion",
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(false);
    });
    fireEvent.change(screen.getByLabelText("Type d’exclusion"), {
      target: { value: "path_prefix" },
    });
    fireEvent.change(screen.getByLabelText("Valeur"), {
      target: { value: "../Secrets" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Ajouter une exclusion" }));
    expect(
      screen.getByText(/chemin relatif sûr/i),
    ).toBeTruthy();
    expect(api.addMonitoringExclusion).toHaveBeenCalledTimes(1);
  });

  it("makes startup reconciliation explicit and blocks another check", async () => {
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue(
      copyDashboard({ startupReconciliationPending: true }),
    );

    render(<MonitoringView workspaceId="workspace-10" />);
    expect(
      await screen.findByText("Mise à jour au démarrage en cours"),
    ).toBeTruthy();
    expect(
      (
        screen.getByRole("button", {
          name: "Vérifier maintenant",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(
      screen.getByText(/Aucune modification de fichiers n’est effectuée/),
    ).toBeTruthy();
  });

  it("runs and cancels a check while surfacing sanitized command errors", async () => {
    const completed = copyDashboard({
      counts: {
        filesAnalyzed: 20,
        readyToOrganize: 5,
        needsReview: 2,
        pendingProposals: 6,
        pendingJobs: 0,
      },
    });
    vi.mocked(api.runMonitoringCycle).mockResolvedValue(completed);
    vi.mocked(api.cancelMonitoring).mockRejectedValue(
      new Error("/Users/example/private failure"),
    );

    render(<MonitoringView workspaceId="workspace-10" />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Vérifier maintenant" }),
    );
    await waitFor(() => {
      expect(api.runMonitoringCycle).toHaveBeenCalledWith("workspace-10");
      expect(screen.getByLabelText("Fichiers analysés").textContent).toContain(
        "20",
      );
    });

    fireEvent.click(
      await screen.findByRole("button", { name: "Annuler la vérification" }),
    );
    await waitFor(() => {
      expect(api.cancelMonitoring).toHaveBeenCalledWith("workspace-10");
      expect(
        screen.getByText(/n’a pas pu être terminée|Surveillance interrompue/i),
      ).toBeTruthy();
      expect(screen.queryByText(/\/Users\/example\/private/)).toBeNull();
    });
  });

  it("keeps cancellation available while a manual check is running", async () => {
    vi.mocked(api.runMonitoringCycle).mockImplementation(
      () => new Promise<MonitoringDashboard>(() => {}),
    );

    render(<MonitoringView workspaceId="workspace-10" />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Vérifier maintenant" }),
    );

    const cancel = await screen.findByRole("button", {
      name: "Annuler la vérification",
    });
    expect((cancel as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(cancel);

    await waitFor(() => {
      expect(api.cancelMonitoring).toHaveBeenCalledWith("workspace-10");
    });
  });
});
