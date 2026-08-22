import { useEffect, useId, useRef, useState } from "react";
import { recordBetaMetric } from "./betaMetrics";

type OnboardingStep = 0 | 1 | 2;

export type OnboardingViewProps = {
  selectedPath?: string | null;
  onSelectFolder: () => void | Promise<void>;
  selectBusy?: boolean;
  wholeComputerBusy?: boolean;
  onComplete: () => void;
  onStartWholeComputer: (kinds: string[]) => void | Promise<void>;
};

const STEPS: Array<{ title: string; body: string }> = [
  {
    title: "ZEMO range vos fichiers, pas vos applications.",
    body: "Le rangement concerne vos documents personnels. Les programmes restent en place.",
  },
  {
    title: "Vous voyez toujours un aperçu avant le rangement.",
    body: "Rien n’est déplacé tant que vous n’avez pas cliqué sur Appliquer le rangement.",
  },
  {
    title: "Vous pouvez annuler après le rangement.",
    body: "Si le résultat ne vous convient pas, un bouton Annuler remet vos fichiers comme avant.",
  },
];

export function OnboardingView({
  onSelectFolder,
  selectBusy = false,
  wholeComputerBusy = false,
  onComplete,
  onStartWholeComputer,
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

  function completeOnboarding() {
    recordBetaMetric("onboarding_completed", { success: true });
    onComplete();
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
              <>
                <button
                  type="button"
                  disabled={selectBusy || wholeComputerBusy}
                  onClick={() => {
                    completeOnboarding();
                    void onSelectFolder();
                  }}
                >
                  Choisir les dossiers
                </button>
                <button
                  className="primary"
                  type="button"
                  disabled={wholeComputerBusy}
                  onClick={() => {
                    recordBetaMetric("onboarding_completed", { success: true });
                    recordBetaMetric("organization_started");
                    void onStartWholeComputer([]);
                  }}
                >
                  {wholeComputerBusy ? "Analyse…" : "Ranger mon ordinateur"}
                </button>
              </>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
