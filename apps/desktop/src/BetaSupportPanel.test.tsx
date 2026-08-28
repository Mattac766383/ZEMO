// @vitest-environment jsdom

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { buildBetaSupportReport } from "./betaSupport";
import { BetaSupportPanel } from "./BetaSupportPanel";
import type { SystemStatus } from "./types";

const system: SystemStatus = {
  localFirst: true,
  readOnlyScan: false,
  networkDisabled: true,
  applyEnabled: true,
  version: "0.1.0-beta.1",
  recoveryRequired: false,
  journalLocked: false,
  journalDiagnostics: [],
};

describe("BetaSupportPanel", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("builds a privacy-safe report with build and safety state", () => {
    const report = buildBetaSupportReport({
      system,
      entries: [],
      generatedAt: new Date("2026-08-28T18:00:00.000Z"),
    });

    expect(report).toContain("ZEMO private beta support v1");
    expect(report).toContain("app_version=0.1.0-beta.1");
    expect(report).toContain("local_first=true");
    expect(report).toContain("network_disabled=true");
    expect(report).toContain("journal_diagnostics=0");
    expect(report).not.toContain("Documents");
    expect(report).not.toContain(".pdf");
  });

  it("rejects an unexpected version string instead of copying arbitrary text into the report", () => {
    const report = buildBetaSupportReport({
      system: { ...system, version: "secret/path client.pdf" },
      entries: [],
      generatedAt: new Date("2026-08-28T18:00:00.000Z"),
    });

    expect(report).toContain("app_version=unknown");
    expect(report).not.toContain("secret/path");
    expect(report).not.toContain("client.pdf");
  });

  it("copies the diagnostic when clipboard access is available", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(<BetaSupportPanel system={system} />);
    fireEvent.click(screen.getByText("Support bêta"));
    fireEvent.click(screen.getByRole("button", { name: "Copier le diagnostic" }));

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: "Diagnostic copié ✓" })).toBeTruthy();
  });
});
