import { useMemo, useState } from "react";
import { buildBetaSupportReport } from "./betaSupport";
import type { SystemStatus } from "./types";

type BetaSupportPanelProps = {
  system: SystemStatus | null;
};

type CopyState = "idle" | "copied" | "failed";

export function BetaSupportPanel({ system }: BetaSupportPanelProps) {
  const [copyState, setCopyState] = useState<CopyState>("idle");
  const report = useMemo(() => buildBetaSupportReport({ system }), [system]);

  async function copyDiagnostic() {
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error("clipboard unavailable");
      }
      await navigator.clipboard.writeText(report);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
  }

  return (
    <details className="beta-support-panel">
      <summary>Support bêta</summary>
      <p>
        Si quelque chose ne fonctionne pas, copiez ce diagnostic et envoyez-le avec une courte
        description de ce que vous faisiez. Il ne contient aucun nom de fichier, chemin, contenu ou
        texte de recherche.
      </p>
      <textarea
        aria-label="Diagnostic bêta ZEMO"
        readOnly
        rows={12}
        value={report}
        onFocus={(event) => event.currentTarget.select()}
      />
      <div className="home-secondary-actions">
        <button type="button" onClick={() => void copyDiagnostic()}>
          {copyState === "copied" ? "Diagnostic copié ✓" : "Copier le diagnostic"}
        </button>
      </div>
      {copyState === "failed" ? (
        <p role="status">
          La copie automatique n’est pas disponible. Sélectionnez le texte ci-dessus puis copiez-le
          manuellement.
        </p>
      ) : null}
      <p>
        Pour un retour utile, ajoutez simplement : 1) ce que vous vouliez faire, 2) ce qui s’est
        passé, 3) une capture d’écran si elle aide à comprendre.
      </p>
    </details>
  );
}
