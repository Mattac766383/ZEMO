import { Component, type ErrorInfo, type ReactNode } from "react";
import { recordBetaMetric } from "./betaMetrics";

type Props = {
  children: ReactNode;
  onReload?: () => void;
};

type State = {
  crashed: boolean;
};

export class BetaErrorBoundary extends Component<Props, State> {
  state: State = { crashed: false };

  static getDerivedStateFromError(): State {
    return { crashed: true };
  }

  componentDidCatch(_error: Error, _info: ErrorInfo): void {
    recordBetaMetric("ui_crash", { success: false });
  }

  private reload = () => {
    if (this.props.onReload) {
      this.props.onReload();
      return;
    }
    window.location.reload();
  };

  render() {
    if (!this.state.crashed) {
      return this.props.children;
    }

    return (
      <main className="scanner-shell">
        <section className="notice-banner notice-banner--critical" role="alert">
          <div>
            <strong>ZEMO a rencontré un problème d’affichage.</strong>
            <span>
              Vos fichiers n’ont pas été modifiés par cette erreur d’interface.
            </span>
            <span className="notice-banner__hint">
              Rechargez ZEMO. Si le problème revient, notez simplement l’étape où il s’est produit.
            </span>
          </div>
          <button type="button" onClick={this.reload}>
            Recharger ZEMO
          </button>
        </section>
      </main>
    );
  }
}
