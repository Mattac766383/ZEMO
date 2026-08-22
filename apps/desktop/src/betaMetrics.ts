export type BetaMetricEvent =
  | "onboarding_completed"
  | "organization_started"
  | "organization_completed"
  | "undo_completed"
  | "search_opened"
  | "ui_crash";

export type BetaMetricFields = {
  count?: number;
  durationMs?: number;
  success?: boolean;
};

export type BetaMetricEntry = {
  event: BetaMetricEvent;
  at: string;
  count?: number;
  durationMs?: number;
  success?: boolean;
};

const STORAGE_KEY = "zemo.beta.metrics.v1";
const MAX_ENTRIES = 200;

function safeInteger(value: unknown): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return undefined;
  }
  return Math.max(0, Math.min(Math.round(value), 1_000_000_000));
}

function parseEntries(raw: string | null): BetaMetricEntry[] {
  if (!raw) {
    return [];
  }
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((entry): entry is BetaMetricEntry => {
      return Boolean(
        entry &&
          typeof entry === "object" &&
          typeof entry.event === "string" &&
          typeof entry.at === "string",
      );
    });
  } catch {
    return [];
  }
}

/**
 * Stores only coarse product events on this device.
 *
 * Important privacy contract: the API deliberately has no field for file names,
 * paths, search queries, extracted text, identities, client/supplier names, or
 * any other user content. Unknown runtime fields are ignored because the stored
 * object is rebuilt explicitly from this allow-list.
 */
export function recordBetaMetric(
  event: BetaMetricEvent,
  fields: BetaMetricFields = {},
): void {
  if (typeof window === "undefined" || !window.localStorage) {
    return;
  }

  const entry: BetaMetricEntry = {
    event,
    at: new Date().toISOString(),
  };
  const count = safeInteger(fields.count);
  const durationMs = safeInteger(fields.durationMs);
  if (count !== undefined) {
    entry.count = count;
  }
  if (durationMs !== undefined) {
    entry.durationMs = durationMs;
  }
  if (typeof fields.success === "boolean") {
    entry.success = fields.success;
  }

  try {
    const previous = parseEntries(window.localStorage.getItem(STORAGE_KEY));
    const next = [...previous.slice(-(MAX_ENTRIES - 1)), entry];
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Metrics must never block or degrade the product experience.
  }
}

export function readBetaMetrics(): BetaMetricEntry[] {
  if (typeof window === "undefined" || !window.localStorage) {
    return [];
  }
  try {
    return parseEntries(window.localStorage.getItem(STORAGE_KEY));
  } catch {
    return [];
  }
}

export function clearBetaMetrics(): void {
  if (typeof window === "undefined" || !window.localStorage) {
    return;
  }
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Best effort only.
  }
}
