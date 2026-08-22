// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BetaErrorBoundary } from "./BetaErrorBoundary";
import { clearBetaMetrics, readBetaMetrics } from "./betaMetrics";

function BrokenView(): never {
  throw new Error("render failed");
}

describe("BetaErrorBoundary", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  beforeEach(() => {
    window.localStorage.clear();
    clearBetaMetrics();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  it("shows a recoverable user-facing fallback instead of a blank screen", () => {
    const onReload = vi.fn();

    render(
      <BetaErrorBoundary onReload={onReload}>
        <BrokenView />
      </BetaErrorBoundary>,
    );

    expect(
      screen.getByText("ZEMO a rencontré un problème d’affichage."),
    ).toBeTruthy();
    expect(
      screen.getByText("Vos fichiers n’ont pas été modifiés par cette erreur d’interface."),
    ).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Recharger ZEMO" }));
    expect(onReload).toHaveBeenCalledTimes(1);
  });

  it("records only a coarse local crash event", () => {
    render(
      <BetaErrorBoundary onReload={() => undefined}>
        <BrokenView />
      </BetaErrorBoundary>,
    );

    expect(readBetaMetrics()).toEqual([
      expect.objectContaining({
        event: "ui_crash",
        success: false,
      }),
    ]);
  });
});
