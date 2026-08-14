// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { ExecutionPanel } from "./ExecutionPanel";
import type {
  ExecutionDetail,
  ExecutionProgress,
  ExecutionSession,
  OrganizationProposal,
  RecoveryAssessment,
} from "./types";

let progressHandler: ((progress: ExecutionProgress) => void) | undefined;

vi.mock("./api", () => ({
  approveExecution: vi.fn(),
  cancelExecution: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
  getExecutionStatus: vi.fn(),
  getSystemStatus: vi.fn(),
  listExecutionHistory: vi.fn(),
  pauseExecution: vi.fn(),
  prepareExecution: vi.fn(),
  recoverExecution: vi.fn(),
  rollbackExecution: vi.fn(),
  selectAndRegisterRoot: vi.fn(),
  setOrganizationProposalStatus: vi.fn(),
  startExecution: vi.fn(),
  subscribeExecutionProgress: vi.fn((handler) => {
    progressHandler = handler;
    return Promise.resolve(() => undefined);
  }),
}));

const proposal: OrganizationProposal = {
  id: "proposal-8",
  revisionId: "revision-8",
  workspaceId: "workspace-8",
  rootId: "root-8",
  sourceScanId: "scan-8",
  revision: 8,
  status: "APPROVED_FOR_FUTURE_APPLY",
  engineVersion: "8.0.0",
  policyVersion: "8.0.0",
  sourceSemanticVersion: "5.0.0",
  sourceRelationshipVersion: "6.0.0",
  createdAt: "2026-08-11T10:00:00Z",
  updatedAt: "2026-08-11T10:01:00Z",
  summary: {
    filesAnalyzed: 4,
    proposedMoves: 3,
    proposedRenames: 2,
    unchanged: 1,
    needsReview: 1,
    unresolved: 0,
    conflicts: 0,
    duplicateNoAction: 0,
    highConfidence: 3,
    mediumConfidence: 1,
    lowConfidence: 0,
    averageDepth: 2,
    maximumDepth: 3,
  },
  change: {
    destinationsChanged: 0,
    filesAdded: 4,
    conflictsResolved: 0,
    movedToReview: 1,
  },
  nodes: [],
  operations: [],
};

const summary = {
  affectedFiles: 4,
  foldersToCreate: 2,
  filesToMove: 3,
  filesToRename: 2,
  filesUnchanged: 1,
  conflicts: 0,
  needsReview: 1,
  preflightOk: 3,
  applied: 0,
  blocked: 1,
  skipped: 0,
  failed: 0,
  rolledBack: 0,
  rollbackBlocked: 0,
  rollbackFailed: 0,
};

function session(
  status: string,
  recoveryState = "RECOVERY_NOT_REQUIRED",
): ExecutionSession {
  return {
    id: "execution-8",
    planId: "plan-8",
    proposalId: proposal.id,
    proposalRevision: proposal.revision,
    workspaceId: proposal.workspaceId,
    status,
    recoveryState,
    planDigest: "a".repeat(64),
    approvedOperationCount: 4,
    consentState:
      status === "AWAITING_CONFIRMATION"
        ? "PENDING"
        : status === "APPROVED"
          ? "ATTESTED"
          : "CONSUMED",
    consentIssuedAtUnixMs: status === "AWAITING_CONFIRMATION" ? null : 1_786_445_000_000,
    consentExpiresAtUnixMs: status === "AWAITING_CONFIRMATION" ? null : 1_786_445_600_000,
    consentAttestedAtUnixMs: status === "AWAITING_CONFIRMATION" ? null : 1_786_445_000_100,
    consentConsumedAtUnixMs:
      status === "AWAITING_CONFIRMATION" || status === "APPROVED"
        ? null
        : 1_786_445_000_200,
    consentInvalidatedAtUnixMs: null,
    summary: { ...summary },
    currentOperation: null,
    rollbackAvailable: status === "COMPLETED",
    confirmationPhraseRequired: true,
    createdAt: "2026-08-11T10:02:00Z",
    approvedAt: status === "AWAITING_CONFIRMATION" ? null : "2026-08-11T10:03:00Z",
    startedAt: null,
    completedAt: status === "COMPLETED" ? "2026-08-11T10:04:00Z" : null,
    rolledBackAt: null,
    error: null,
  };
}

function detail(status: string, recoveryState?: string): ExecutionDetail {
  return {
    session: session(status, recoveryState),
    operations: [],
  };
}

function recoveryAssessment(
  overrides: Partial<RecoveryAssessment> = {},
): RecoveryAssessment {
  return {
    executionId: "execution-8",
    state: "RECOVERY_AMBIGUOUS",
    affectedCount: 2,
    notStarted: 0,
    applied: 1,
    ambiguous: 1,
    verifiedAppliedItems: [
      {
        operationId: "operation-applied",
        direction: "FORWARD",
        item: "Organized/applied.pdf",
        reason: "Exact post-apply identity and hash were verified.",
      },
    ],
    verifiedNotStartedItems: [],
    ambiguousItems: [
      {
        operationId: "operation-ambiguous",
        direction: "FORWARD",
        item: "Inbox/uncertain.pdf → Organized/uncertain.pdf",
        reason: "Neither endpoint matches the durable request fingerprint.",
      },
    ],
    rollbackAvailable: false,
    executorSessions: [
      {
        sessionId: "a".repeat(64),
        executionId: "execution-8",
        planId: "plan-8",
        purpose: "FORWARD",
        coordinatorPid: 41,
        childPid: 42,
        openedAtUnixMs: 1_786_445_000_000,
      },
    ],
    executorRequests: [
      {
        requestId: "b".repeat(64),
        sessionId: "a".repeat(64),
        operationId: "operation-ambiguous",
        direction: "FORWARD",
        requestSequence: 1,
        intentEventSequence: 3,
        state: "AMBIGUOUS",
      },
    ],
    journalDiagnostics: { locked: false, diagnostics: [] },
    message: "Recovery is ambiguous; no further mutation is allowed.",
    ...overrides,
  };
}

describe("Milestone 8 safety-gated execution UI", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    progressHandler = undefined;
    vi.mocked(api.getSystemStatus).mockResolvedValue({
      localFirst: true,
      readOnlyScan: false,
      networkDisabled: true,
      applyEnabled: true,
      applyGateReason: "Approved execution service only",
      displayLabel: "Local encrypted execution",
      version: "8.0.0",
      recoveryRequired: false,
      journalLocked: false,
      journalDiagnostics: [],
    });
    vi.mocked(api.listExecutionHistory).mockResolvedValue([]);
    vi.mocked(api.prepareExecution).mockResolvedValue(detail("AWAITING_CONFIRMATION"));
    vi.mocked(api.approveExecution).mockResolvedValue(detail("APPROVED"));
    vi.mocked(api.cancelExecution).mockResolvedValue(true);
    vi.mocked(api.pauseExecution).mockResolvedValue(true);
    vi.mocked(api.rollbackExecution).mockResolvedValue(detail("ROLLED_BACK"));
    vi.mocked(api.setOrganizationProposalStatus).mockResolvedValue(proposal);
  });

  it("uses native approval and requires only the large-batch UX phrase", async () => {
    let completeStart: ((value: ExecutionDetail) => void) | undefined;
    vi.mocked(api.startExecution).mockImplementation(
      () =>
        new Promise((resolve) => {
          completeStart = resolve;
        }),
    );
    render(<ExecutionPanel workspaceId={proposal.workspaceId} proposal={proposal} />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Appliquer l’organisation" }),
    );
    expect(await screen.findByText("Appliquer cette organisation ?")).toBeTruthy();
    expect(
      screen.getByText(/Vos fichiers seront déplacés selon l’organisation affichée/),
    ).toBeTruthy();
    const apply = screen.getByRole("button", {
      name: "Appliquer",
    }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/saisissez exactement ORGANIZE/i), {
      target: { value: "organize" },
    });
    expect(apply.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/saisissez exactement ORGANIZE/i), {
      target: { value: "ORGANIZE" },
    });
    expect(apply.disabled).toBe(false);
    fireEvent.click(apply);
    await waitFor(() => {
      expect(api.approveExecution).toHaveBeenCalledWith("execution-8", "ORGANIZE");
    });

    act(() => {
      progressHandler?.({
        executionId: "execution-8",
        status: "RUNNING",
        completed: 1,
        total: 3,
        applied: 1,
        blocked: 1,
        skipped: 0,
        failed: 0,
        current: "Organized/invoice.pdf",
      });
    });
    expect(await screen.findByText("Organisation en cours")).toBeTruthy();
    expect(screen.getByText("1 / 3 fichiers")).toBeTruthy();

    const completed = detail("COMPLETED");
    completed.session.summary.applied = 3;
    await act(async () => {
      completeStart?.(completed);
    });
    expect(await screen.findByText("Organisation terminée")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Annuler les changements" }),
    ).toBeTruthy();
  });

  it("surfaces ambiguous startup recovery and disables mutation controls", async () => {
    const ambiguous = session("RECOVERY_AMBIGUOUS", "RECOVERY_AMBIGUOUS");
    vi.mocked(api.listExecutionHistory).mockResolvedValue([ambiguous]);
    vi.mocked(api.getExecutionStatus).mockResolvedValue({
      session: ambiguous,
      operations: [],
    });
    vi.mocked(api.getSystemStatus).mockResolvedValue({
      localFirst: true,
      readOnlyScan: false,
      networkDisabled: true,
      applyEnabled: true,
      applyGateReason: "Recovery review required",
      displayLabel: "Local encrypted execution",
      version: "8.0.0",
      recoveryRequired: true,
      journalLocked: false,
      journalDiagnostics: [],
    });
    vi.mocked(api.recoverExecution).mockResolvedValue(recoveryAssessment());

    render(<ExecutionPanel workspaceId={proposal.workspaceId} proposal={proposal} />);
    expect(
      await screen.findByText("Une organisation a été interrompue."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Examiner" }));
    expect(screen.queryByRole("button", { name: /^Continuer$/i })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Appliquer l’organisation" }),
    ).toBeNull();
    fireEvent.click(screen.getByText("Détails techniques"));
    expect(await screen.findByText("Verified applied operations")).toBeTruthy();
    expect(screen.getByText("Unresolved / ambiguous")).toBeTruthy();
    expect(screen.getByText("Inspect ambiguous items")).toBeTruthy();
    fireEvent.click(screen.getByText("Inspect ambiguous items"));
    expect(
      screen.getByText(/Neither endpoint matches the durable request fingerprint/),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /rollback verified/i })).toBeNull();
  });

  it("keeps invalidated consent fail-closed in the renderer", async () => {
    const invalidated = detail("AWAITING_CONFIRMATION");
    invalidated.session.consentState = "INVALIDATED";
    invalidated.session.consentInvalidatedAtUnixMs = 1_786_445_000_300;
    vi.mocked(api.listExecutionHistory).mockResolvedValue([invalidated.session]);
    vi.mocked(api.getExecutionStatus).mockResolvedValue(invalidated);

    render(<ExecutionPanel workspaceId={proposal.workspaceId} proposal={proposal} />);
    expect(
      await screen.findByText(/Cette confirmation n’est plus valable/),
    ).toBeTruthy();
    const apply = screen.getByRole("button", {
      name: "Appliquer",
    }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    expect(api.approveExecution).not.toHaveBeenCalled();
  });

  it("offers rollback, never blind resume, after recovery is safely available", async () => {
    const available = session("RECOVERY_AVAILABLE", "RECOVERY_AVAILABLE");
    available.rollbackAvailable = true;
    vi.mocked(api.listExecutionHistory).mockResolvedValue([available]);
    vi.mocked(api.getExecutionStatus).mockResolvedValue({
      session: available,
      operations: [],
    });
    vi.mocked(api.recoverExecution).mockResolvedValue(
      recoveryAssessment({
        state: "RECOVERY_AVAILABLE",
        affectedCount: 1,
        ambiguous: 0,
        ambiguousItems: [],
        rollbackAvailable: true,
        message: "Verified applied operations can be rolled back.",
      }),
    );

    render(<ExecutionPanel workspaceId={proposal.workspaceId} proposal={proposal} />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Examiner",
      }),
    );
    await waitFor(() => {
      expect(api.recoverExecution).toHaveBeenCalled();
    });
    const rollback = (
      await screen.findAllByRole("button", {
        name: "Annuler les changements",
        hidden: true,
      })
    )[0];
    expect(rollback).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Continuer$/i })).toBeNull();
    fireEvent.click(rollback);
    expect(
      await screen.findByText(
        /Les fichiers seront replacés à leur emplacement précédent/,
      ),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole("dialog").querySelector(".danger-action") as HTMLButtonElement,
    );
    await waitFor(() => {
      expect(api.rollbackExecution).toHaveBeenCalledWith("execution-8");
    });
  });

  it("shows authenticated-journal diagnostics without repair or mutation actions", async () => {
    vi.mocked(api.getSystemStatus).mockResolvedValue({
      localFirst: true,
      readOnlyScan: false,
      networkDisabled: true,
      applyEnabled: false,
      applyGateReason: "Authenticated execution journal diagnostics are unresolved.",
      displayLabel: "Local encrypted execution",
      version: "8.1.0",
      recoveryRequired: true,
      journalLocked: true,
      journalDiagnostics: [
        {
          scope: "external",
          executionId: "execution-8",
          code: "external_journal_authentication_failed",
          message: "The encrypted recovery journal failed authentication.",
          detectedAtUnixMs: 1_786_445_000_000,
          recoveryAvailable: false,
          rollbackAvailable: false,
        },
      ],
    });

    render(<ExecutionPanel workspaceId={proposal.workspaceId} proposal={proposal} />);

    expect(
      await screen.findByText("Authenticated execution journal locked"),
    ).toBeTruthy();
    expect(
      screen.getByText(/no repair was attempted/i),
    ).toBeTruthy();
    expect(
      screen.getByText(/external_journal_authentication_failed/),
    ).toBeTruthy();
    expect(
      (screen.getByRole("button", {
        name: "Appliquer l’organisation",
      }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(
      screen.queryByRole("button", { name: /rollback|continue|repair/i }),
    ).toBeNull();
  });

  it("offers re-authorization when macOS folder access is lost", async () => {
    vi.mocked(api.getSystemStatus).mockRejectedValue(
      "macOS n’autorise plus l’accès à ce dossier.",
    );
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-8",
      displayLabel: "sandbox",
      selectedPath: "/tmp/supremacy-m18-step2-sandbox",
    });

    render(<ExecutionPanel workspaceId={proposal.workspaceId} proposal={proposal} />);
    expect(
      await screen.findByText("macOS n’autorise plus l’accès à ce dossier."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Réautoriser" }));
    await waitFor(() => {
      expect(api.selectAndRegisterRoot).toHaveBeenCalledWith(proposal.workspaceId);
    });
  });
});
