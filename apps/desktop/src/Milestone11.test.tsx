// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { RulesPreferencesView } from "./RulesPreferencesView";
import type {
  LocalRule,
  OrganizationPreferences,
  RuleSuggestion,
  RulesPreferencesState,
} from "./types";

vi.mock("./api", () => ({
  acceptLocalRuleSuggestion: vi.fn(),
  createLocalRule: vi.fn(),
  deleteLocalRule: vi.fn(),
  dismissLocalRuleSuggestion: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
  getRulesPreferences: vi.fn(),
  recomputeRulesProposal: vi.fn(),
  reorderLocalRules: vi.fn(),
  setLocalRuleEnabled: vi.fn(),
  storeLocalOrganizationPreferences: vi.fn(),
  updateLocalRule: vi.fn(),
}));

const maliciousExplanation =
  '<img src="x" onerror="window.uploadFiles()">Keep this as text</img>';

const preferences: OrganizationPreferences = {
  clientFirst: true,
  includeYearFolders: true,
  maximumDepth: 4,
  minimumGroupSize: 2,
  keepPhotosInsideProjects: true,
  supplierInvoicesInsideProjects: true,
  namingLanguage: "en",
  preserveExistingFolders: true,
  personalRootName: "Personal",
  businessRootName: "Business",
  renameTemplate: "{date}_{party}_{document_type}_{identifier}",
  reviewThreshold: 0.7,
};

const rules: LocalRule[] = [
  {
    id: "rule-1",
    workspaceId: "workspace-11",
    name: "Project supplier invoices",
    explanation: maliciousExplanation,
    enabled: true,
    position: 0,
    origin: "user_created",
    sourceSuggestionId: null,
    conditions: [
      { field: "document_type", operator: "equals", value: "invoice" },
      { field: "project", operator: "exists", value: null },
    ],
    action: { kind: "prefer_project_location" },
    createdAt: "2026-08-11T10:00:00Z",
    updatedAt: "2026-08-11T10:00:00Z",
  },
  {
    id: "rule-2",
    workspaceId: "workspace-11",
    name: "No year folders for photos",
    explanation: "Photos remain in a shallow project hierarchy.",
    enabled: false,
    position: 1,
    origin: "accepted_suggestion",
    sourceSuggestionId: "suggestion-old",
    conditions: [
      { field: "document_type", operator: "equals", value: "photo" },
    ],
    action: { kind: "use_year_folders", enabled: false },
    createdAt: "2026-08-11T10:01:00Z",
    updatedAt: "2026-08-11T10:01:00Z",
  },
];

const suggestions: RuleSuggestion[] = [
  {
    id: "suggestion-1",
    workspaceId: "workspace-11",
    signature: "supplier-point-p",
    title: "Point P invoices repeatedly corrected",
    explanation: "3 matching local corrections. No rule exists yet.",
    evidenceCount: 3,
    status: "pending",
    proposedRule: {
      name: "Classify Point P as supplier",
      explanation: "Created only if you explicitly accept this suggestion.",
      enabled: true,
      conditions: [
        { field: "any_party", operator: "equals", value: "Point P" },
      ],
      action: {
        kind: "classify_party",
        party: "Point P",
        role: "supplier",
      },
    },
    acceptedRuleId: null,
    createdAt: "2026-08-11T10:02:00Z",
    updatedAt: "2026-08-11T10:02:00Z",
  },
  {
    id: "suggestion-2",
    workspaceId: "workspace-11",
    signature: "tax-documents",
    title: "Tax destination repeated",
    explanation: "4 matching local corrections. No rule exists yet.",
    evidenceCount: 4,
    status: "pending",
    proposedRule: {
      name: "Keep taxes under Personal",
      explanation: "Created only after consent.",
      enabled: true,
      conditions: [
        { field: "document_type", operator: "equals", value: "tax_document" },
      ],
      action: {
        kind: "set_destination",
        segments: ["Personal", "Administrative", "Taxes"],
      },
    },
    acceptedRuleId: null,
    createdAt: "2026-08-11T10:03:00Z",
    updatedAt: "2026-08-11T10:03:00Z",
  },
];

function state(): RulesPreferencesState {
  return {
    rules: rules.map((rule) => ({
      ...rule,
      conditions: rule.conditions.map((condition) => ({ ...condition })),
      action:
        rule.action.kind === "set_destination"
          ? { ...rule.action, segments: [...rule.action.segments] }
          : { ...rule.action },
    })),
    suggestions: suggestions.map((suggestion) => ({
      ...suggestion,
      proposedRule: { ...suggestion.proposedRule },
    })),
    preferences: { ...preferences },
  };
}

describe("Milestone 11 local rules and preferences UI", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getRulesPreferences).mockResolvedValue(state());
    vi.mocked(api.createLocalRule).mockResolvedValue(rules[0]);
    vi.mocked(api.updateLocalRule).mockResolvedValue(rules[0]);
    vi.mocked(api.setLocalRuleEnabled).mockResolvedValue(rules[0]);
    vi.mocked(api.deleteLocalRule).mockResolvedValue(true);
    vi.mocked(api.reorderLocalRules).mockResolvedValue(rules);
    vi.mocked(api.storeLocalOrganizationPreferences).mockResolvedValue(preferences);
    vi.mocked(api.acceptLocalRuleSuggestion).mockResolvedValue(rules[0]);
    vi.mocked(api.dismissLocalRuleSuggestion).mockResolvedValue(suggestions[1]);
    vi.mocked(api.recomputeRulesProposal).mockResolvedValue({
      revision: 12,
    } as Awaited<ReturnType<typeof api.recomputeRulesProposal>>);
  });

  it("renders inspectable rules and local-only safety copy without interpreting HTML", async () => {
    const { container } = render(
      <RulesPreferencesView workspaceId="workspace-11" />,
    );

    expect(
      await screen.findByRole("heading", { name: "Préférences de rangement" }),
    ).toBeTruthy();
    expect(screen.getByText(/n.appliquent jamais de modifications de fichiers/i)).toBeTruthy();
    expect(screen.getByText(maliciousExplanation)).toBeTruthy();
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText("Créée par vous")).toBeTruthy();
    expect(screen.getByText("Suggestion acceptée")).toBeTruthy();
    expect(screen.getByText("IF Project exists")).toBeTruthy();
    expect(
      screen.getByText("THEN Préférer l’emplacement du projet lié"),
    ).toBeTruthy();
    expect(
      screen.getByText(/devient une règle automatiquement/i),
    ).toBeTruthy();
    expect(
      screen.queryByRole("button", {
        name: /^(apply|execute|organize files)$/i,
      }),
    ).toBeNull();
  });

  it("creates and validates typed rules with safe destination segments", async () => {
    render(<RulesPreferencesView workspaceId="workspace-11" />);
    await screen.findByRole("heading", { name: "Créer une règle" });

    fireEvent.change(screen.getByLabelText("Condition 1 champ"), {
      target: { value: "source_path" },
    });
    expect(
      (screen.getByLabelText("Condition 1 opérateur") as HTMLSelectElement)
        .value,
    ).toBe("starts_with");
    fireEvent.change(screen.getByLabelText("Condition 1 champ"), {
      target: { value: "document_type" },
    });

    fireEvent.click(screen.getByRole("button", { name: "CRÉER LA RÈGLE" }));
    expect(
      screen.getByText("Un nom et une raison en langage simple sont requis."),
    ).toBeTruthy();
    expect(api.createLocalRule).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("Nom de la règle"), {
      target: { value: "Keep taxes together" },
    });
    fireEvent.change(screen.getByLabelText("Why this rule exists"), {
      target: { value: "This is my explicit local filing policy." },
    });
    fireEvent.change(screen.getByLabelText("Action"), {
      target: { value: "set_destination" },
    });
    fireEvent.change(
      screen.getByLabelText("Destination (separate folders with /)"),
      { target: { value: "../Secrets" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "CRÉER LA RÈGLE" }));
    expect(
      screen.getByText("Les dossiers de destination doivent être des noms relatifs sûrs."),
    ).toBeTruthy();
    expect(api.createLocalRule).not.toHaveBeenCalled();

    fireEvent.change(
      screen.getByLabelText("Destination (separate folders with /)"),
      { target: { value: "Personal / Administrative / Taxes" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "CRÉER LA RÈGLE" }));
    await waitFor(() => {
      expect(api.createLocalRule).toHaveBeenCalledWith("workspace-11", {
        name: "Keep taxes together",
        explanation: "This is my explicit local filing policy.",
        enabled: true,
        conditions: [
          {
            field: "document_type",
            operator: "equals",
            value: "invoice",
          },
        ],
        action: {
          kind: "set_destination",
          segments: ["Personal", "Administrative", "Taxes"],
        },
      });
    });
  });

  it("edits, enables, deletes, and deterministically reorders rules", async () => {
    render(<RulesPreferencesView workspaceId="workspace-11" />);
    await screen.findByText("Project supplier invoices");

    fireEvent.click(screen.getAllByRole("button", { name: "Move down" })[0]);
    await waitFor(() => {
      expect(api.reorderLocalRules).toHaveBeenCalledWith("workspace-11", [
        "rule-2",
        "rule-1",
      ]);
    });

    fireEvent.click(screen.getByLabelText("Activer No year folders for photos"));
    await waitFor(() => {
      expect(api.setLocalRuleEnabled).toHaveBeenCalledWith(
        "workspace-11",
        "rule-2",
        true,
      );
    });
    await waitFor(() => {
      expect(
        (screen.getAllByRole("button", { name: "Edit" })[0] as HTMLButtonElement)
          .disabled,
      ).toBe(false);
    });

    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    expect(
      await screen.findByRole("heading", { name: "Modifier la règle" }),
    ).toBeTruthy();
    const name = screen.getByLabelText("Nom de la règle");
    fireEvent.change(name, {
      target: { value: "Invoices stay with their project" },
    });
    fireEvent.click(screen.getByRole("button", { name: "ENREGISTRER LA RÈGLE" }));
    await waitFor(() => {
      expect(api.updateLocalRule).toHaveBeenCalledWith(
        "workspace-11",
        "rule-1",
        expect.objectContaining({
          name: "Invoices stay with their project",
        }),
      );
    });

    fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    await waitFor(() => {
      expect(api.deleteLocalRule).toHaveBeenCalledWith(
        "workspace-11",
        "rule-1",
      );
    });
  });

  it("requires consent for suggestions and supports accepting or dismissing them", async () => {
    render(<RulesPreferencesView workspaceId="workspace-11" />);
    await screen.findByText("Point P invoices repeatedly corrected");

    expect(api.createLocalRule).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getAllByRole("button", {
        name: "ACCEPTER ET CRÉER LA RÈGLE",
      })[0],
    );
    await waitFor(() => {
      expect(api.acceptLocalRuleSuggestion).toHaveBeenCalledWith(
        "workspace-11",
        "suggestion-1",
      );
    });
    expect(api.createLocalRule).not.toHaveBeenCalled();

    fireEvent.click(screen.getAllByRole("button", { name: "Ignorer" })[1]);
    await waitFor(() => {
      expect(api.dismissLocalRuleSuggestion).toHaveBeenCalledWith(
        "workspace-11",
        "suggestion-2",
      );
    });
  });

  it("stores complete local preferences and explicitly recomputes the preview", async () => {
    render(<RulesPreferencesView workspaceId="workspace-11" />);
    await screen.findByDisplayValue("Personal");

    fireEvent.change(screen.getByLabelText("Personal root"), {
      target: { value: "Private" },
    });
    fireEvent.change(screen.getByLabelText("Business root"), {
      target: { value: "Company" },
    });
    fireEvent.change(screen.getByLabelText("Folder language"), {
      target: { value: "fr" },
    });
    fireEvent.change(screen.getByLabelText("Maximum depth"), {
      target: { value: "5" },
    });
    fireEvent.change(screen.getByLabelText(/Review threshold/), {
      target: { value: "0.82" },
    });
    fireEvent.click(screen.getByRole("button", { name: "ENREGISTRER LES PRÉFÉRENCES" }));
    await waitFor(() => {
      expect(api.storeLocalOrganizationPreferences).toHaveBeenCalledWith(
        "workspace-11",
        expect.objectContaining({
          personalRootName: "Private",
          businessRootName: "Company",
          namingLanguage: "fr",
          maximumDepth: 5,
          reviewThreshold: 0.82,
        }),
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "RECALCULER L’APERÇU" }));
    await waitFor(() => {
      expect(api.recomputeRulesProposal).toHaveBeenCalledWith("workspace-11");
      expect(
        screen.getByText(
          "Preview recomputed as revision 12; unrelated manual overrides were preserved.",
        ),
      ).toBeTruthy();
    });
  });
});
