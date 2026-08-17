import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { resetOnboardingCompleted } from "../onboardingStorage";

function createMemoryStorage(): Storage {
  const memory = new Map<string, string>();
  return {
    get length() {
      return memory.size;
    },
    clear() {
      memory.clear();
    },
    getItem(key) {
      return memory.has(key) ? (memory.get(key) ?? null) : null;
    },
    key(index) {
      return [...memory.keys()][index] ?? null;
    },
    removeItem(key) {
      memory.delete(key);
    },
    setItem(key, value) {
      memory.set(String(key), String(value));
    },
  };
}

function installStorage(name: "localStorage" | "sessionStorage") {
  const current = window[name];
  if (current && typeof current.clear === "function" && typeof current.getItem === "function") {
    return;
  }
  Object.defineProperty(window, name, {
    configurable: true,
    enumerable: true,
    value: createMemoryStorage(),
  });
}

installStorage("localStorage");
installStorage("sessionStorage");

afterEach(() => {
  cleanup();
  try {
    window.localStorage.clear();
  } catch {
    installStorage("localStorage");
  }
  try {
    window.sessionStorage.clear();
  } catch {
    installStorage("sessionStorage");
  }
  resetOnboardingCompleted();
});
