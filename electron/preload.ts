import { contextBridge, ipcRenderer } from 'electron';

interface SupremacyAPI {
  readFile: (path: string) => Promise<{ ok: boolean; content?: string; error?: string }>;
  writeFile: (path: string, content: string) => Promise<{ ok: boolean; error?: string }>;
  listDir: (path: string) => Promise<{
    ok: boolean;
    entries?: { name: string; isDirectory: boolean }[];
    error?: string;
  }>;
  execCommand: (command: string, cwd?: string) => Promise<{
    ok: boolean;
    stdout?: string;
    stderr?: string;
    error?: string;
  }>;
  askPermission: (action: string, details: string) => Promise<boolean>;
  pickFolder: () => Promise<string | null>;
  pickFile: () => Promise<string | null>;
  getHome: () => Promise<string>;
  storageLoad: () => Promise<Record<string, unknown> | null>;
  storageSave: (data: Record<string, unknown>) => Promise<boolean>;
  httpFetch: (
    url: string,
    options?: { method?: string; headers?: Record<string, string>; body?: string },
  ) => Promise<{ ok: boolean; status: number; body: string; error?: string }>;
  showNotification: (title: string, body: string) => Promise<boolean>;
}

const api: SupremacyAPI = {
  readFile: (path) => ipcRenderer.invoke('fs:read', path),
  writeFile: (path, content) => ipcRenderer.invoke('fs:write', path, content),
  listDir: (path) => ipcRenderer.invoke('fs:list', path),
  execCommand: (command, cwd) => ipcRenderer.invoke('shell:exec', command, cwd),
  askPermission: (action, details) => ipcRenderer.invoke('permission:ask', action, details),
  pickFolder: () => ipcRenderer.invoke('dialog:pickFolder'),
  pickFile: () => ipcRenderer.invoke('dialog:pickFile'),
  getHome: () => ipcRenderer.invoke('app:getHome'),
  storageLoad: () => ipcRenderer.invoke('storage:load'),
  storageSave: (data) => ipcRenderer.invoke('storage:save', data),
  httpFetch: (url, options) => ipcRenderer.invoke('http:fetch', url, options),
  showNotification: (title, body) => ipcRenderer.invoke('notify:show', title, body),
};

contextBridge.exposeInMainWorld('supremacy', api);
