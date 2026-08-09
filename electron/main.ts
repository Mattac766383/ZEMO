import { app, BrowserWindow, ipcMain, dialog, Notification } from 'electron';
import path from 'path';
import { fileURLToPath } from 'url';
import { exec } from 'child_process';
import { promisify } from 'util';
import fs from 'fs/promises';

const execAsync = promisify(exec);
const __dirname = path.dirname(fileURLToPath(import.meta.url));

function getStoragePath() {
  return path.join(app.getPath('userData'), 'supremacy-data.json');
}

let mainWindow: BrowserWindow | null = null;

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1500,
    height: 950,
    minWidth: 1100,
    minHeight: 750,
    backgroundColor: '#050510',
    titleBarStyle: 'hiddenInset',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  if (process.env.VITE_DEV_SERVER_URL) {
    mainWindow.loadURL(process.env.VITE_DEV_SERVER_URL);
  } else {
    mainWindow.loadFile(path.join(__dirname, '../dist/index.html'));
  }
}

app.whenReady().then(createWindow);
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow();
});

// ─── Persistence ─────────────────────────────────────────────────
ipcMain.handle('storage:load', async () => {
  try {
    const raw = await fs.readFile(getStoragePath(), 'utf-8');
    return JSON.parse(raw);
  } catch {
    return null;
  }
});

ipcMain.handle('storage:save', async (_, data: Record<string, unknown>) => {
  try {
    await fs.writeFile(getStoragePath(), JSON.stringify(data, null, 2), 'utf-8');
    return true;
  } catch {
    return false;
  }
});

// ─── IPC: fichiers ───────────────────────────────────────────────
ipcMain.handle('fs:read', async (_, filePath: string) => {
  try {
    const content = await fs.readFile(filePath, 'utf-8');
    return { ok: true, content };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
});

ipcMain.handle('fs:write', async (_, filePath: string, content: string) => {
  try {
    await fs.writeFile(filePath, content, 'utf-8');
    return { ok: true };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
});

ipcMain.handle('fs:list', async (_, dirPath: string) => {
  try {
    const entries = await fs.readdir(dirPath, { withFileTypes: true });
    return {
      ok: true,
      entries: entries.map((e) => ({
        name: e.name,
        isDirectory: e.isDirectory(),
      })),
    };
  } catch (e) {
    return { ok: false, error: String(e) };
  }
});

ipcMain.handle('shell:exec', async (_, command: string, cwd?: string) => {
  try {
    const { stdout, stderr } = await execAsync(command, {
      cwd: cwd || process.env.HOME,
      timeout: 120000,
      maxBuffer: 10 * 1024 * 1024,
    });
    return { ok: true, stdout, stderr };
  } catch (e: unknown) {
    const err = e as { stdout?: string; stderr?: string; message?: string };
    return {
      ok: false,
      stdout: err.stdout ?? '',
      stderr: err.stderr ?? '',
      error: err.message ?? String(e),
    };
  }
});

ipcMain.handle('permission:ask', async (_, action: string, details: string) => {
  const result = await dialog.showMessageBox(mainWindow!, {
    type: 'warning',
    buttons: ['Autoriser', 'Refuser'],
    defaultId: 1,
    title: 'Supremacy — Permission requise',
    message: action,
    detail: details,
  });
  return result.response === 0;
});

ipcMain.handle('dialog:pickFolder', async () => {
  const result = await dialog.showOpenDialog(mainWindow!, {
    properties: ['openDirectory'],
  });
  return result.canceled ? null : result.filePaths[0];
});

ipcMain.handle('dialog:pickFile', async () => {
  const result = await dialog.showOpenDialog(mainWindow!, {
    properties: ['openFile'],
  });
  return result.canceled ? null : result.filePaths[0];
});

ipcMain.handle('app:getHome', () => process.env.HOME ?? '');

// ─── IPC: HTTP (APIs externes, évite CORS) ───────────────────────
ipcMain.handle(
  'http:fetch',
  async (
    _,
    url: string,
    options?: { method?: string; headers?: Record<string, string>; body?: string },
  ) => {
    try {
      const res = await fetch(url, {
        method: options?.method ?? 'GET',
        headers: options?.headers,
        body: options?.body,
      });
      const body = await res.text();
      return { ok: res.ok, status: res.status, body };
    } catch (e) {
      return { ok: false, status: 0, body: '', error: String(e) };
    }
  },
);

ipcMain.handle('notify:show', async (_, title: string, body: string) => {
  if (!Notification.isSupported()) return false;
  new Notification({ title, body }).show();
  return true;
});
