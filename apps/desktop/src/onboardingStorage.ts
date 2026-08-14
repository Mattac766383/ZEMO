const ONBOARDING_COMPLETED_KEY = "supremacy.onboarding.v1.completed";

/** Process-local fallback when localStorage is unavailable or non-persistent. */
let memoryCompleted = false;

function readStoredFlag(): boolean | null {
  try {
    const value = window.localStorage.getItem(ONBOARDING_COMPLETED_KEY);
    if (value === "1") {
      return true;
    }
    if (value === null || value === "0") {
      return false;
    }
  } catch {
    // private mode / restricted environments
  }
  return null;
}

function writeStoredFlag(completed: boolean): void {
  try {
    if (completed) {
      window.localStorage.setItem(ONBOARDING_COMPLETED_KEY, "1");
    } else {
      window.localStorage.removeItem(ONBOARDING_COMPLETED_KEY);
    }
  } catch {
    // Ignore quota / private-mode failures; memory fallback remains.
  }
}

export function isOnboardingCompleted(): boolean {
  const stored = readStoredFlag();
  if (stored === true) {
    memoryCompleted = true;
    return true;
  }
  if (stored === false && memoryCompleted) {
    // Storage may be non-persistent (e.g. broken test localStorage).
    return true;
  }
  if (stored === false) {
    return false;
  }
  return memoryCompleted;
}

export function markOnboardingCompleted(): void {
  memoryCompleted = true;
  writeStoredFlag(true);
}

export function resetOnboardingCompleted(): void {
  memoryCompleted = false;
  writeStoredFlag(false);
}
