const ORGANIZE_RESULT_KEY = "supremacy.oneclick.v1.lastResult";

export type LastOrganizeResult = {
  filesMoved: number;
  executionIds: string[];
  completedAt: string;
};

let memoryResult: LastOrganizeResult | null = null;

export function readLastOrganizeResult(): LastOrganizeResult | null {
  try {
    const raw = window.localStorage.getItem(ORGANIZE_RESULT_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as LastOrganizeResult;
      if (typeof parsed.filesMoved === "number") {
        memoryResult = parsed;
        return parsed;
      }
    }
  } catch {
    // private mode
  }
  return memoryResult;
}

export function writeLastOrganizeResult(result: LastOrganizeResult): void {
  memoryResult = result;
  try {
    window.localStorage.setItem(ORGANIZE_RESULT_KEY, JSON.stringify(result));
  } catch {
    // ignore
  }
}

export function clearLastOrganizeResult(): void {
  memoryResult = null;
  try {
    window.localStorage.removeItem(ORGANIZE_RESULT_KEY);
  } catch {
    // ignore
  }
}
