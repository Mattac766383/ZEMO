from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1 match, got {count}")
    return text.replace(old, new, 1)


def replace_test(text: str, name: str, body: str) -> str:
    pattern = re.compile(
        r'  it\("' + re.escape(name) + r'", async \(\) => \{.*?\n  \}\);',
        re.S,
    )
    updated, count = pattern.subn(body.rstrip(), text, count=1)
    if count != 1:
        raise RuntimeError(f"test {name!r}: expected 1 match, got {count}")
    return updated


# Keep advanced inventory selection read-only/manual. The primary Rangement action
# gets its own explicit selected-folder One-Click pipeline.
path = "apps/desktop/src/App.tsx"
text = read(path)
pattern = re.compile(
    r'  async function handleSelectFolder\(\) \{.*?\n  \}\n\n  async function handleStartScan\(\) \{',
    re.S,
)
replacement = '''  async function handleSelectFolder() {
    clearError();
    setBusy("select");
    try {
      let activeWorkspace = workspace;
      if (!activeWorkspace) {
        activeWorkspace = await createWorkspace("Inventaire local");
        setWorkspace(activeWorkspace);
      }
      const selected = await selectAndRegisterRoot(activeWorkspace.id);
      setRoot(selected);
      setOrganizationRootId(selected.id);
      setScan(null);
      setProgress(null);
      setAnalysis(null);
      setAnalysisProgress(null);
      setSemanticAnalysis(null);
      setSemanticProgress(null);
      setContentResults([]);
      setSelectedContent(null);
      setDetailFileId(null);
      setDetailIdentityId(null);
      setFiles([]);
      setDuplicates([]);
      setIssues([]);
      setView(
        ["summary", "files", "duplicates", "errors", "content"].includes(view)
          ? "summary"
          : "home",
      );
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(null);
    }
  }

  async function handleOrganizeSelectedFolder() {
    clearError();
    setBusy("select");
    setWholeComputerBusy(true);
    setWholeComputerProgress(null);
    setAccessSummary(null);
    setAccessProbes([]);
    setOneClickProposals([]);
    setOneClickFilesAnalyzed(0);
    try {
      let activeWorkspace = workspace;
      if (!activeWorkspace) {
        activeWorkspace = await createWorkspace("ZEMO");
        setWorkspace(activeWorkspace);
      }

      const selected = await selectAndRegisterRoot(activeWorkspace.id);
      setRoot(selected);
      setOrganizationRootId(selected.id);
      setScan(null);
      setProgress(null);
      setAnalysis(null);
      setAnalysisProgress(null);
      setSemanticAnalysis(null);
      setSemanticProgress(null);
      setContentResults([]);
      setSelectedContent(null);
      setDetailFileId(null);
      setDetailIdentityId(null);
      setFiles([]);
      setDuplicates([]);
      setIssues([]);
      setOneClickFolders([
        {
          kind: selected.id,
          label: selected.displayLabel || "Dossier sélectionné",
          phase: "scanning",
        },
      ]);
      markOnboardingCompleted();
      setShowOnboarding(false);
      setView("oneclick-scan");

      setWholeComputerProgress("Analyse du dossier choisi…");
      const scanResult = await scanWorkspace(activeWorkspace.id);
      setScan(scanResult);
      setOneClickFilesAnalyzed(scanResult.filesIndexed);

      setWholeComputerProgress("Lecture du contenu…");
      const contentAnalysis = await analyzeContent(scanResult.id);
      setAnalysis(contentAnalysis);

      setWholeComputerProgress("Compréhension des fichiers…");
      const semantic = await analyzeSemantics(scanResult.id);
      setSemanticAnalysis(semantic);

      setWholeComputerProgress("Préparation du rangement…");
      const proposal = await generateOrganizationProposal(
        activeWorkspace.id,
        true,
        selected.id,
        true,
      );
      if (!proposal) {
        throw new Error("Aucune proposition de rangement n’a été générée.");
      }
      setOneClickProposals([proposal]);
      setOneClickFolders([
        {
          kind: selected.id,
          label: selected.displayLabel || "Dossier sélectionné",
          phase: "ready",
          filesIndexed: scanResult.filesIndexed,
        },
      ]);
      setView("oneclick-preview");
    } catch (reason) {
      reportError(reason, "organization");
      setView("home");
    } finally {
      setBusy(null);
      setWholeComputerBusy(false);
      setWholeComputerProgress(null);
    }
  }

  async function handleStartScan() {'''
text, count = pattern.subn(replacement, text, count=1)
if count != 1:
    raise RuntimeError(f"split folder handlers: expected 1 match, got {count}")
text = text.replace(
    'if (action.run === "selectFolder") {\n      void handleSelectFolder();',
    'if (action.run === "selectFolder") {\n      void handleOrganizeSelectedFolder();',
    1,
)
text = text.replace(
    'if (action.run === "ranger") {\n      void handleSelectFolder();',
    'if (action.run === "ranger") {\n      void handleOrganizeSelectedFolder();',
    1,
)
text = text.replace(
    'onSelectFolder={handleSelectFolder}\n          onStartWholeComputer={handleWholeComputer}',
    'onSelectFolder={handleOrganizeSelectedFolder}\n          onStartWholeComputer={handleWholeComputer}',
    1,
)
write(path, text)

# Defensive defaults for legacy UI tests. Individual feature tests can still
# override these mocks with richer fixtures.
DEFAULT_ROOT = '''selectAndRegisterRoot: vi.fn().mockResolvedValue({
    id: "root-selected",
    displayLabel: "Dossier test",
    selectedPath: "/Users/local/Dossier-test",
  }),'''
DEFAULT_PROPOSAL = '''generateOrganizationProposal: vi.fn().mockResolvedValue({
    id: "proposal-selected",
    revisionId: "revision-selected",
    workspaceId: "workspace-1",
    rootId: "root-selected",
    sourceScanId: "scan-1",
    revision: 1,
    status: "READY_FOR_REVIEW",
    engineVersion: "test",
    policyVersion: "test",
    createdAt: "2026-09-05T20:00:00Z",
    updatedAt: "2026-09-05T20:00:00Z",
    summary: {
      filesAnalyzed: 0,
      proposedMoves: 0,
      proposedRenames: 0,
      unchanged: 0,
      needsReview: 0,
      unresolved: 0,
      conflicts: 0,
      highConfidence: 0,
      mediumConfidence: 0,
      lowConfidence: 0,
      duplicateNoAction: 0,
      averageDepth: 0,
      maximumDepth: 0,
    },
    change: {
      destinationsChanged: 0,
      filesAdded: 0,
      conflictsResolved: 0,
      movedToReview: 0,
    },
    nodes: [],
    operations: [],
  }),'''
for test_path in (ROOT / "apps/desktop/src").glob("*.test.tsx"):
    test = test_path.read_text(encoding="utf-8")
    test = test.replace("selectAndRegisterRoot: vi.fn(),", DEFAULT_ROOT)
    test = test.replace("generateOrganizationProposal: vi.fn(),", DEFAULT_PROPOSAL)
    test = test.replace(
        'name: "Vous voyez toujours un aperçu avant le rangement."',
        'name: "Une sélection suffit."',
    )
    # The first-pass text migration turned old-negative assertions into a
    # contradiction. Keep asserting the old whole-PC CTA is absent instead.
    test = test.replace(
        'screen.queryByRole("button", { name: "Choisir un dossier à ranger" })',
        'screen.queryByRole("button", { name: "Ranger mon ordinateur" })',
    )
    test_path.write_text(test, encoding="utf-8")

# Dedicated onboarding + selected-folder privacy tests.
path = "apps/desktop/src/Milestone12_2Ux.test.tsx"
text = read(path)
text = text.replace(
    'describe("Milestone 12.2 zero-friction UX + whole computer", () => {',
    'describe("Milestone 12.2 zero-friction UX + user-selected folders", () => {',
)
text = replace_test(
    text,
    "shows Ranger mon ordinateur and Choisir les dossiers on first run",
    '''  it("shows one explicit selected-folder action on first run", async () => {
    render(<App />);
    expect(
      await screen.findByRole("heading", {
        name: "Vous choisissez ce que ZEMO peut ranger.",
      }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(screen.getByRole("heading", { name: "Une sélection suffit." })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(screen.getByRole("heading", { name: "Vous gardez le contrôle." })).toBeTruthy();
    expect(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Choisir un dossier à ranger",
      }),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Ranger mon ordinateur" })).toBeNull();
    expect(screen.queryByText(/embedding|Granite/i)).toBeNull();
  });''',
)
text = replace_test(
    text,
    "starts one-click organize with the default personal folders",
    '''  it("starts organization only from the folder chosen by the user", async () => {
    const onSelect = vi.fn();
    const onStart = vi.fn();
    render(
      <OnboardingView
        onSelectFolder={onSelect}
        onComplete={vi.fn()}
        onStartWholeComputer={onStart}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Choisir un dossier à ranger" }));
    await waitFor(() => expect(onSelect).toHaveBeenCalledTimes(1));
    expect(onStart).not.toHaveBeenCalled();
  });''',
)
text = replace_test(
    text,
    "runs whole computer with partial permission denial and still previews",
    '''  it("never discovers personal folders automatically in the primary flow", async () => {
    vi.mocked(api.generateOrganizationProposal).mockResolvedValue({
      id: "proposal-1",
      revisionId: "rev-1",
      workspaceId: "workspace-1",
      rootId: "root-selected",
      sourceScanId: "scan-1",
      revision: 1,
      status: "READY_FOR_REVIEW",
      engineVersion: "1",
      policyVersion: "1",
      createdAt: "2026-09-05T20:00:00Z",
      updatedAt: "2026-09-05T20:00:00Z",
      summary: {
        filesAnalyzed: 10,
        proposedMoves: 0,
        proposedRenames: 0,
        unchanged: 10,
        needsReview: 0,
        unresolved: 0,
        conflicts: 0,
        highConfidence: 0,
        mediumConfidence: 0,
        lowConfidence: 0,
        duplicateNoAction: 0,
        averageDepth: 0,
        maximumDepth: 0,
      },
      change: { destinationsChanged: 0, filesAdded: 0, conflictsResolved: 0, movedToReview: 0 },
      nodes: [],
      operations: [],
    });
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Choisir un dossier à ranger" }));

    await waitFor(() => {
      expect(api.selectAndRegisterRoot).toHaveBeenCalledWith("workspace-1");
      expect(api.scanWorkspace).toHaveBeenCalledWith("workspace-1");
      expect(api.analyzeContent).toHaveBeenCalledWith("scan-1");
      expect(api.analyzeSemantics).toHaveBeenCalledWith("scan-1");
      expect(api.generateOrganizationProposal).toHaveBeenCalledWith(
        "workspace-1",
        true,
        "root-selected",
        true,
      );
    });
    expect(api.listUserContentLocations).not.toHaveBeenCalled();
    expect(api.probeUserContentAccess).not.toHaveBeenCalled();
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
    expect(
      await screen.findByRole("heading", { name: /0 fichiers? à ranger/i }),
    ).toBeTruthy();
  });''',
)
write(path, text)

# OneClick product contract: selected folder only, recursive pipeline, no default-root probing.
path = "apps/desktop/src/MilestoneOneClick.test.tsx"
text = read(path)
text = replace_test(
    text,
    "shows a three-step first launch then Ranger mon ordinateur",
    '''  it("shows a three-step first launch ending with user-selected folder access", async () => {
    render(<OnboardingView onSelectFolder={vi.fn()} onComplete={vi.fn()} onStartWholeComputer={vi.fn()} />);
    expect(screen.getByRole("heading", { name: "Vous choisissez ce que ZEMO peut ranger." })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(screen.getByRole("heading", { name: "Une sélection suffit." })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(screen.getByRole("heading", { name: "Vous gardez le contrôle." })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Choisir un dossier à ranger" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Ranger mon ordinateur" })).toBeNull();
  });''',
)
text = replace_test(
    text,
    "walks home → scan → preview → apply → done → undo",
    '''  it("walks selected folder → scan → preview → apply → done → undo", async () => {
    markOnboardingCompleted();
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-desktop",
      displayLabel: "Bureau",
      selectedPath: "/Users/local/Desktop",
    });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Que voulez-vous ranger ?" })).toBeTruthy();
    expect(screen.getByText(/vous choisissez le dossier/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Choisir un dossier à ranger" }));

    await waitFor(() => {
      expect(api.selectAndRegisterRoot).toHaveBeenCalledWith("workspace-1");
      expect(api.scanWorkspace).toHaveBeenCalled();
      expect(api.analyzeContent).toHaveBeenCalledWith("scan-1");
      expect(api.analyzeSemantics).toHaveBeenCalledWith("scan-1");
      expect(api.generateOrganizationProposal).toHaveBeenCalled();
    });
    expect(api.probeUserContentAccess).not.toHaveBeenCalled();
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
    const generateArgs = vi.mocked(api.generateOrganizationProposal).mock.calls[0];
    expect(generateArgs).toEqual(["workspace-1", true, "root-desktop", true]);

    expect(await screen.findByRole("heading", { name: /7 fichiers à ranger/i })).toBeTruthy();
    expect(screen.getByText("Documents")).toBeTruthy();
    expect(screen.getByText("Installateurs")).toBeTruthy();
    expect(screen.queryByText(/confidence|moteur local|journal sequence/i)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Appliquer le rangement" }));
    expect(await screen.findByRole("heading", { name: "Rangement terminé." })).toBeTruthy();
    expect(screen.getByText(/7 fichiers rangés/i)).toBeTruthy();
    expect(screen.getByText(/0 fichier supprimé/i)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Annuler le rangement" }));
    await waitFor(() => expect(api.rollbackExecution).toHaveBeenCalledWith("exec-1"));
  });''',
)
text = replace_test(
    text,
    "asks for in-product authorization instead of dumping the user home",
    '''  it("does not probe Desktop or Documents before the user chooses a folder", async () => {
    markOnboardingCompleted();
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Choisir un dossier à ranger" }));
    await waitFor(() => expect(api.selectAndRegisterRoot).toHaveBeenCalled());
    expect(api.listUserContentLocations).not.toHaveBeenCalled();
    expect(api.probeUserContentAccess).not.toHaveBeenCalled();
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
  });''',
)
text = replace_test(
    text,
    "keeps Autoriser visible when packaged inspect fails as unexpected_error",
    '''  it("stops safely if the native folder selection is refused", async () => {
    markOnboardingCompleted();
    vi.mocked(api.selectAndRegisterRoot).mockRejectedValueOnce(new Error("selection refused"));
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Choisir un dossier à ranger" }));
    await waitFor(() => expect(api.selectAndRegisterRoot).toHaveBeenCalled());
    expect(api.scanWorkspace).not.toHaveBeenCalled();
    expect(api.generateOrganizationProposal).not.toHaveBeenCalled();
  });''',
)
text = replace_test(
    text,
    "scans accessible folders when another folder still needs authorization",
    '''  it("scans only the selected root and generates a consumer proposal for it", async () => {
    markOnboardingCompleted();
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-selected-only",
      displayLabel: "Entreprise",
      selectedPath: "/Users/local/Entreprise",
    });
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Choisir un dossier à ranger" }));
    await waitFor(() => {
      expect(api.generateOrganizationProposal).toHaveBeenCalledWith(
        "workspace-1",
        true,
        "root-selected-only",
        true,
      );
    });
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
    expect(api.probeUserContentAccess).not.toHaveBeenCalled();
  });''',
)
write(path, text)

# Walkthrough: the onboarding CTA now goes straight into the chosen-folder pipeline.
path = "apps/desktop/src/Milestone12Walkthrough.test.tsx"
text = read(path)
text = replace_test(
    text,
    "walks a first-time user from onboarding to search and monitoring",
    '''  it("walks a first-time user from selected-folder onboarding to search", async () => {
    const latest = await api.getLatestOrganizationProposal("workspace-1");
    vi.mocked(api.generateOrganizationProposal).mockResolvedValue(latest as never);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Vous choisissez ce que ZEMO peut ranger." })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(screen.getByRole("heading", { name: "Une sélection suffit." })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Choisir un dossier à ranger" }));

    await waitFor(() => {
      expect(api.selectAndRegisterRoot).toHaveBeenCalledWith("workspace-1");
      expect(api.scanWorkspace).toHaveBeenCalledWith("workspace-1");
      expect(api.generateOrganizationProposal).toHaveBeenCalledWith(
        "workspace-1",
        true,
        "root-1",
        true,
      );
    });
    expect(api.probeUserContentAccess).not.toHaveBeenCalled();
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
    expect(await screen.findByRole("heading", { name: /fichiers? à ranger/i })).toBeTruthy();

    const nav = screen.getByRole("navigation", { name: "Navigation principale" });
    fireEvent.click(within(nav).getByRole("button", { name: "Recherche" }));
    const search = await screen.findByLabelText(/^Recherche$/i);
    fireEvent.change(search, { target: { value: "facture Point P" } });
    await waitFor(() => expect(api.searchLocalFiles).toHaveBeenCalled());
    expect(await screen.findByRole("heading", { name: "facture-point-p.pdf" })).toBeTruthy();
  });''',
)
write(path, text)

print("selected-folder follow-up patch applied")
