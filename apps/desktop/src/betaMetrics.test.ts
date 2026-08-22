// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import {
  buildBetaDiagnosticText,
  clearBetaMetrics,
  readBetaMetrics,
  recordBetaMetric,
} from "./betaMetrics";

describe("betaMetrics privacy contract", () => {
  beforeEach(() => {
    window.localStorage.clear();
    clearBetaMetrics();
  });

  it("stores only allow-listed coarse fields", () => {
    recordBetaMetric(
      "organization_completed",
      {
        count: 42,
        durationMs: 1500,
        success: true,
        path: "/Users/test/Documents/secret.pdf",
        filename: "secret.pdf",
        query: "facture client martin",
        content: "private document text",
      } as never,
    );

    const [entry] = readBetaMetrics();
    expect(entry).toMatchObject({
      event: "organization_completed",
      count: 42,
      durationMs: 1500,
      success: true,
    });

    const serialized = JSON.stringify(entry);
    expect(serialized).not.toContain("secret.pdf");
    expect(serialized).not.toContain("/Users/test/Documents");
    expect(serialized).not.toContain("facture client martin");
    expect(serialized).not.toContain("private document text");
  });

  it("keeps the local history bounded", () => {
    for (let index = 0; index < 250; index += 1) {
      recordBetaMetric("search_opened", { count: index });
    }

    const entries = readBetaMetrics();
    expect(entries).toHaveLength(200);
    expect(entries.at(-1)?.count).toBe(249);
  });

  it("sanitizes invalid numeric values", () => {
    recordBetaMetric("organization_completed", {
      count: Number.POSITIVE_INFINITY,
      durationMs: -100,
      success: false,
    });

    expect(readBetaMetrics()[0]).toMatchObject({
      event: "organization_completed",
      durationMs: 0,
      success: false,
    });
    expect(readBetaMetrics()[0]?.count).toBeUndefined();
  });

  it("builds a shareable diagnostic containing only counters", () => {
    recordBetaMetric("organization_started");
    recordBetaMetric("organization_completed", { count: 12, success: true });
    recordBetaMetric("search_opened");

    const diagnostic = buildBetaDiagnosticText();
    expect(diagnostic).toContain("ZEMO beta diagnostic v1");
    expect(diagnostic).toContain("organization_completed=1");
    expect(diagnostic).toContain("files_organized=12");
    expect(diagnostic).toContain("search_opened=1");
    expect(diagnostic).not.toContain("/");
    expect(diagnostic).not.toContain(".pdf");
  });
});
