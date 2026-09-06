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


# Onboarding: one explicit permission surface, chosen by the user.
onboarding = '''import { useEffect, useId, useRef, useState } from "react";
import { recordBetaMetric } from "./betaMetrics";

type OnboardingStep = 0 | 1 | 2;

export type OnboardingViewProps = {
  selectedPath?: string | null;
  onSelectFolder: () => void | Promise<void>;
  selectBusy?: boolean;
  wholeComputerBusy?: boolean;
  onComplete: () => void;
  onStartWholeComputer?: (kinds: string[]) => void | Promise<void>;
};

const STEPS: Array<{ title: string; body: string }> = [
  {
    title: "Vous choisissez ce que ZEMO peut ranger.",
    body: "ZEMO n’accède qu’au dossier que vous sélectionnez. Rien d’autre n’est parcouru.",
  },
  {
    title: "Une sélection suffit.",
    body: "ZEMO détecte automatiquement tous les fichiers et sous-dossiers de l’emplacement choisi.",
  },
  {
    title: "Vous gardez le contrôle.",
    body: "ZEMO montre un aperçu avant tout déplacement, puis vous pouvez annuler le rangement.",
  },
];

export function OnboardingView({
  onSelectFolder,
  selectBusy = false,
  onComplete,
}: OnboardingViewProps) {
  const [step, setStep] = useState<OnboardingStep>(0);
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const node = dialogRef.current;
    if (!node) {
      return;
    }
    const focusable = node.querySelector<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    );
    focusable?.focus();
  }, [step]);

  const last = step === 2;

  function startSelectedFolderFlow() {
    recordBetaMetric("onboarding_completed", { success: true });
    recordBetaMetric("organization_started");
    onComplete();
    void onSelectFolder();
  }

  return (
    <div className="onboarding-overlay">
      <div
        ref={dialogRef}
        className="onboarding-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <p className="onboarding-eyebrow">ZEMO · Premiers pas</p>
        <p className="onboarding-step-indicator">
          {step + 1} / {STEPS.length}
        </p>
        <section className="onboarding-step" aria-labelledby={titleId}>
          <h1 id={titleId}>{STEPS[step].title}</h1>
          <p>{STEPS[step].body}</p>
          <p>
            Analyse locale : les noms, chemins, contenus et recherches de vos fichiers ne sont pas envoyés par la télémétrie bêta.
          </p>
          <div className="onboarding-actions">
            {step > 0 ? (
              <button
                type="button"
                onClick={() => setStep((current) => (current - 1) as OnboardingStep)}
              >
                Retour
              </button>
            ) : null}
            {!last ? (
              <button
                className="primary"
                type="button"
                onClick={() => setStep((current) => (current + 1) as OnboardingStep)}
              >
                Continuer
              </button>
            ) : (
              <button
                className="primary"
                type="button"
                disabled={selectBusy}
                onClick={startSelectedFolderFlow}
              >
                {selectBusy ? "Ouverture…" : "Choisir un dossier à ranger"}
              </button>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
'''
write("apps/desktop/src/OnboardingView.tsx", onboarding)

# Home: never advertise or trigger whole-computer access.
path = "apps/desktop/src/HomeDashboard.tsx"
text = read(path)
text = replace_once(
    text,
    '''  if (input.organized) {\n    return { label: "Relancer le rangement", run: "ranger" };\n  }\n  return { label: "Ranger mon ordinateur", run: "ranger" };''',
    '''  if (input.organized) {\n    return { label: "Ranger un autre dossier", run: "selectFolder" };\n  }\n  return { label: "Choisir un dossier à ranger", run: "selectFolder" };''',
    "primary action",
)
text = text.replace("Votre ordinateur est rangé.", "Votre dossier est rangé.")
text = text.replace("Relancer le rangement", "Ranger un autre dossier")
text = text.replace("Votre ordinateur est en bazar ?", "Que voulez-vous ranger ?")
text = text.replace(
    "ZEMO range vos fichiers personnels sans toucher à vos applications.",
    "Vous choisissez le dossier. ZEMO ne touche à rien d’autre.",
)
text = text.replace(
    "Avant tout changement, ZEMO vous montre seulement les dossiers qu’il veut créer.",
    "ZEMO analyse automatiquement tous les fichiers et sous-dossiers du dossier choisi, puis vous montre un aperçu avant tout changement.",
)
text = text.replace("Ranger mon ordinateur", "Choisir un dossier à ranger")
write(path, text)

# App: selected folder immediately enters the real scan -> extraction -> semantics -> proposal flow.
path = "apps/desktop/src/App.tsx"
text = read(path)
text = replace_once(
    text,
    '''    if (action.run === "ranger") {\n      void handleWholeComputer([]);\n      return;\n    }''',
    '''    if (action.run === "ranger") {\n      void handleSelectFolder();\n      return;\n    }''',
    "legacy ranger safety redirect",
)
pattern = re.compile(r'''  async function handleSelectFolder\(\) \{.*?\n  \}\n\n  async function handleStartScan\(\) \{''', re.S)
replacement = '''  async function handleSelectFolder() {
    clearError();
    setBusy("select");
    setWholeComputerBusy(true);
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
      setAccessSummary(null);
      setAccessProbes([]);
      setOneClickProposals([]);
      setOneClickFilesAnalyzed(0);
      setOneClickFolders([
        {
          kind: selected.id,
          label: selected.displayLabel || "Dossier sélectionné",
          phase: "scanning",
        },
      ]);
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
    } finally {
      setBusy(null);
      setWholeComputerBusy(false);
      setWholeComputerProgress(null);
    }
  }

  async function handleStartScan() {'''
text, count = pattern.subn(replacement, text, count=1)
if count != 1:
    raise RuntimeError(f"handleSelectFolder: expected 1 match, got {count}")
write(path, text)

# Update user-facing frontend expectations to the new consent model.
for test_path in (ROOT / "apps/desktop/src").glob("*.test.tsx"):
    test = test_path.read_text(encoding="utf-8")
    test = test.replace("ZEMO range vos fichiers, pas vos applications.", "Vous choisissez ce que ZEMO peut ranger.")
    test = test.replace("Vous pouvez annuler après le rangement.", "Vous gardez le contrôle.")
    test = test.replace("Choisir les dossiers", "Choisir un dossier à ranger")
    test = test.replace("Ranger mon ordinateur", "Choisir un dossier à ranger")
    test = test.replace("Votre ordinateur est en bazar ?", "Que voulez-vous ranger ?")
    test = test.replace("Votre ordinateur est rangé.", "Votre dossier est rangé.")
    test = test.replace("Relancer le rangement", "Ranger un autre dossier")
    test = test.replace(
        "range vos fichiers personnels sans toucher à vos applications",
        "choisissez le dossier. ZEMO ne touche à rien d’autre",
    )
    test_path.write_text(test, encoding="utf-8")

# Onboarding now has exactly one final CTA. Remove the stale assertion that expected
# both the whole-computer and manual-folder actions.
path = "apps/desktop/src/Milestone12Onboarding.test.tsx"
text = read(path)
text = text.replace(
    '''    expect(\n      screen.getByRole("button", { name: "Choisir un dossier à ranger" }),\n    ).toBeTruthy();\n''',
    "",
    1,
)
write(path, text)

print("user-selected folder flow patch applied")
