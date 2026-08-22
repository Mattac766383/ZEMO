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
const EVENTS: readonly BetaMetricEvent[] = [
  "onboarding_completed",
  "organization_started",
  "organization_completed",
  "undo_completed",
  "search_opened",
  "ui_crash",
];
const EVENT_SET: ReadonlySet<BetaMetricEvent> = new Set(EVENTS);

function getStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }
  try {
    return window.localStorage ?? null;
  } catch {
    return null;
  }
}

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
          EVENT_SET.has(entry.event as BetaMetricEvent) &&
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
  const storage = getStorage();
  if (!storage) {
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
    const previous = parseEntries(storage.getItem(STORAGE_KEY));
    const next = [...previous.slice(-(MAX_ENTRIES - 1)), entry];
    storage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Metrics must never block or degrade the product experience.
  }
}

export function readBetaMetrics(): BetaMetricEntry[] {
  const storage = getStorage();
  if (!storage) {
    return [];
  }
  try {
    return parseEntries(storage.getItem(STORAGE_KEY));
  } catch {
    return [];
  }
}

export function buildBetaDiagnosticText(
  entries: BetaMetricEntry[] = readBetaMetrics(),
): string {
  const counts = new Map<BetaMetricEvent, number>(
    EVENTS.map((event) => [event, 0] as const),
  );
  let filesOrganized = 0;

  for (const entry of entries) {
    counts.set(entry.event, (counts.get(entry.event) ?? 0) + 1);
    if (entry.event === "organization_completed") {
      filesOrganized += entry.count ?? 0;
    }
  }

  return [
    "ZEMO beta diagnostic v1",
    `events=${entries.length}`,
    `onboarding_completed=${counts.get("onboarding_completed") ?? 0}`,
    `organization_started=${counts.get("organization_started") ?? 0}`,
    `organization_completed=${counts.get("organization_completed") ?? 0}`,
    `files_organized=${filesOrganized}`,
    `undo_completed=${counts.get("undo_completed") ?? 0}`,
    `search_opened=${counts.get("search_opened") ?? 0}`,
    `ui_crash=${counts.get("ui_crash") ?? 0}`,
  ].join("\n");
}

export function clearBetaMetrics(): void {
  const storage = getStorage();
  if (!storage) {
    return;
  }
  try {
    storage.removeItem(STORAGE_KEY);
  } catch {
    // Best effort only.
  }
}
