import { useEffect, useId, useRef, useState } from "react";
import { listUserContentLocations } from "./api";
import type { UserContentLocation } from "./types";

type OnboardingMode = "welcome" | "whole-computer" | "custom-folder";

export type OnboardingViewProps = {
  selectedPath?: string | null;
  onSelectFolder: () => void | Promise<void>;
  selectBusy?: boolean;
  wholeComputerBusy?: boolean;
  onComplete: () => void;
  onStartWholeComputer: (kinds: string[]) => void | Promise<void>;
};

export function OnboardingView({
  selectedPath,
  onSelectFolder,
  selectBusy = false,
  wholeComputerBusy = false,
  onComplete,
  onStartWholeComputer,
}: OnboardingViewProps) {
  const [mode, setMode] = useState<OnboardingMode>("welcome");
  const [locations, setLocations] = useState<UserContentLocation[] | null>(null);
  const [locationsError, setLocationsError] = useState<string | null>(null);
  const [selectedKinds, setSelectedKinds] = useState<string[]>([]);
  const [customizing, setCustomizing] = useState(false);
  const [permissionReady, setPermissionReady] = useState(false);
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
  }, [mode, customizing, permissionReady]);

  useEffect(() => {
    if (mode !== "whole-computer") {
      return;
    }
    let active = true;
    setLocationsError(null);
    void listUserContentLocations()
      .then((next) => {
        if (!active) {
          return;
        }
        setLocations(next);
        setSelectedKinds(
          next
            .filter((item) => item.exists && item.recommended)
            .map((item) => item.kind),
        );
      })
      .catch(() => {
        if (active) {
          setLocationsError(
            "Impossible de préparer la liste des dossiers à analyser.",
          );
          setLocations([]);
        }
      });
    return () => {
      active = false;
    };
  }, [mode]);

  const selectedLocations =
    locations?.filter((item) => selectedKinds.includes(item.kind)) ?? [];
  const canStartWholeComputer =
    selectedLocations.some((item) => item.exists) && !wholeComputerBusy;

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

        {mode === "welcome" ? (
          <section className="onboarding-step" aria-labelledby={titleId}>
            <h1 id={titleId}>
              Organisez et retrouvez vos fichiers automatiquement.
            </h1>
            <p>
              Vos fichiers sont analysés localement. Nous préparons une
              organisation proposée — rien n’est déplacé automatiquement.
            </p>
            <ul className="onboarding-points">
              <li>Analyse locale sur votre Mac</li>
              <li>Vous choisissez ce qui est analysé</li>
              <li>Les fichiers système et les applications restent exclus</li>
            </ul>
            <div className="onboarding-primary-choices">
              <button
                className="primary"
                type="button"
                onClick={() => {
                  setMode("whole-computer");
                  setPermissionReady(false);
                  setCustomizing(false);
                }}
              >
                Organiser mon ordinateur
              </button>
              <button
                type="button"
                onClick={() => {
                  setMode("custom-folder");
                  setPermissionReady(false);
                }}
              >
                Choisir des dossiers
              </button>
            </div>
            <p className="onboarding-note">
              Recommandé : analyser Bureau, Documents, Téléchargements et Images.
            </p>
          </section>
        ) : null}

        {mode === "whole-computer" ? (
          <section className="onboarding-step" aria-labelledby={titleId}>
            <h1 id={titleId}>Organiser mon ordinateur</h1>
            {!permissionReady ? (
              <>
                <p>
                  Pour analyser vos documents, l’application a besoin d’accéder
                  uniquement aux emplacements que vous choisissez.
                </p>
                <p>
                  L’analyse s’effectue localement sur votre Mac. Aucun fichier
                  n’est déplacé.
                </p>
                <div className="onboarding-actions">
                  <button type="button" onClick={() => setMode("welcome")}>
                    Retour
                  </button>
                  <button
                    className="primary"
                    type="button"
                    onClick={() => setPermissionReady(true)}
                  >
                    Continuer
                  </button>
                </div>
              </>
            ) : (
              <>
                <p>Nous allons analyser :</p>
                {locationsError ? (
                  <p className="inline-error" role="alert">
                    {locationsError}
                  </p>
                ) : null}
                {!locations ? (
                  <p role="status">Préparation des emplacements…</p>
                ) : (
                  <ul className="onboarding-scope-list">
                    {(customizing ? locations : selectedLocations).map(
                      (location) => {
                        const checked = selectedKinds.includes(location.kind);
                        return (
                          <li key={location.kind}>
                            {customizing ? (
                              <label className="onboarding-scope-item">
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  disabled={!location.exists}
                                  onChange={(event) => {
                                    setSelectedKinds((current) =>
                                      event.target.checked
                                        ? [...current, location.kind]
                                        : current.filter(
                                            (kind) => kind !== location.kind,
                                          ),
                                    );
                                  }}
                                />
                                <span>
                                  {location.displayLabel}
                                  {!location.exists
                                    ? " — indisponible"
                                    : !location.readable
                                      ? " — accès à vérifier"
                                      : ""}
                                </span>
                              </label>
                            ) : (
                              <span className="onboarding-scope-item">
                                ✓ {location.displayLabel}
                              </span>
                            )}
                          </li>
                        );
                      },
                    )}
                  </ul>
                )}
                <p className="onboarding-note">
                  Les fichiers système, les applications et les données internes
                  de l’app sont exclus. Le disque entier n’est jamais parcouru.
                </p>
                <div className="onboarding-actions">
                  <button
                    type="button"
                    onClick={() => {
                      if (customizing) {
                        setCustomizing(false);
                        return;
                      }
                      setMode("welcome");
                    }}
                  >
                    Retour
                  </button>
                  {!customizing ? (
                    <button
                      type="button"
                      onClick={() => setCustomizing(true)}
                      disabled={!locations || locations.length === 0}
                    >
                      Personnaliser
                    </button>
                  ) : null}
                  <button
                    className="primary"
                    type="button"
                    disabled={!canStartWholeComputer}
                    onClick={() => {
                      void onStartWholeComputer(selectedKinds);
                    }}
                  >
                    {wholeComputerBusy
                      ? "Analyse…"
                      : "Commencer l’analyse"}
                  </button>
                </div>
              </>
            )}
          </section>
        ) : null}

        {mode === "custom-folder" ? (
          <section className="onboarding-step" aria-labelledby={titleId}>
            <h1 id={titleId}>Choisir des dossiers</h1>
            {!permissionReady ? (
              <>
                <p>
                  Autoriser l’accès à vos fichiers : l’application n’accède qu’au
                  dossier que vous sélectionnez, pour une analyse locale.
                </p>
                <div className="onboarding-actions">
                  <button type="button" onClick={() => setMode("welcome")}>
                    Retour
                  </button>
                  <button
                    className="primary"
                    type="button"
                    onClick={() => setPermissionReady(true)}
                  >
                    Continuer
                  </button>
                </div>
              </>
            ) : (
              <>
                <p>
                  Sélectionnez un dossier à analyser — par exemple Documents ou
                  un dossier professionnel.
                </p>
                <div className="onboarding-folder-actions">
                  <button
                    className="primary"
                    type="button"
                    disabled={selectBusy}
                    onClick={() => {
                      void onSelectFolder();
                    }}
                  >
                    {selectBusy ? "Sélection…" : "Sélectionner un dossier"}
                  </button>
                  <div className="selected-path">
                    <span>Dossier sélectionné</span>
                    <code>
                      {selectedPath ??
                        "Aucun pour l’instant — vous pourrez choisir plus tard"}
                    </code>
                  </div>
                </div>
                <div className="onboarding-actions">
                  <button type="button" onClick={() => setMode("welcome")}>
                    Retour
                  </button>
                  <button
                    className="primary"
                    type="button"
                    onClick={onComplete}
                  >
                    {selectedPath ? "Continuer" : "Passer pour l’instant"}
                  </button>
                </div>
              </>
            )}
          </section>
        ) : null}
      </div>
    </div>
  );
}
