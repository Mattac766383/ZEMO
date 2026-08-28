import { buildBetaDiagnosticText, readBetaMetrics, type BetaMetricEntry } from "./betaMetrics";
import type { SystemStatus } from "./types";

export type BetaSupportReportInput = {
  system: SystemStatus | null;
  entries?: BetaMetricEntry[];
  generatedAt?: Date;
};

function safeVersion(value?: string | null): string {
  const normalized = value?.trim() ?? "";
  return /^[A-Za-z0-9._+-]{1,64}$/.test(normalized) ? normalized : "unknown";
}

function bool(value: boolean | undefined): string {
  return value ? "true" : "false";
}

/**
 * Builds a support report that is intentionally limited to application state
 * and coarse beta counters. It must never contain file names, paths, search
 * queries, extracted content, identities, client names or other user content.
 */
export function buildBetaSupportReport({
  system,
  entries = readBetaMetrics(),
  generatedAt = new Date(),
}: BetaSupportReportInput): string {
  return [
    "ZEMO private beta support v1",
    `generated_at=${generatedAt.toISOString()}`,
    `app_version=${safeVersion(system?.version)}`,
    `local_first=${bool(system?.localFirst)}`,
    `network_disabled=${bool(system?.networkDisabled)}`,
    `read_only_scan=${bool(system?.readOnlyScan)}`,
    `apply_enabled=${bool(system?.applyEnabled)}`,
    `recovery_required=${bool(system?.recoveryRequired)}`,
    `journal_locked=${bool(system?.journalLocked)}`,
    `journal_diagnostics=${system?.journalDiagnostics.length ?? 0}`,
    "",
    buildBetaDiagnosticText(entries),
  ].join("\n");
}
