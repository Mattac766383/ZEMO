import { useEffect, useState } from 'react';
import Editor from '@monaco-editor/react';
import { useStore } from '../stores/appStore';

function detectLanguage(filename: string): string {
  const ext = filename.split('.').pop()?.toLowerCase();
  const map: Record<string, string> = {
    ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript',
    py: 'python', html: 'html', css: 'css', json: 'json', md: 'markdown',
    rs: 'rust', go: 'go', sql: 'sql', sh: 'shell', yaml: 'yaml', yml: 'yaml',
  };
  return map[ext ?? ''] ?? 'plaintext';
}

export function CodeEditor() {
  const {
    editorFile, editorContent, editorLanguage, workspacePath,
    setEditorContent, setEditorFile,
  } = useStore();
  const [files, setFiles] = useState<string[]>([]);

  useEffect(() => {
    if (!workspacePath || !window.supremacy) return;
    window.supremacy.listDir(workspacePath).then((r) => {
      if (r.ok && r.entries) {
        setFiles(r.entries.filter((e) => !e.isDirectory).map((e) => e.name));
      }
    });
  }, [workspacePath, editorFile]);

  const openFile = async (name: string) => {
    if (!workspacePath || !window.supremacy) return;
    const path = `${workspacePath}/${name}`;
    const r = await window.supremacy.readFile(path);
    if (r.ok) setEditorFile(path, r.content ?? '', detectLanguage(name));
  };

  const saveFile = async () => {
    if (!editorFile || !window.supremacy) return;
    const allowed = await window.supremacy.askPermission('Sauvegarder fichier', editorFile);
    if (!allowed) return;
    await window.supremacy.writeFile(editorFile, editorContent);
  };

  const pickFile = async () => {
    if (!window.supremacy) return;
    const path = await window.supremacy.pickFile();
    if (!path) return;
    const r = await window.supremacy.readFile(path);
    if (r.ok) setEditorFile(path, r.content ?? '', detectLanguage(path));
  };

  return (
    <div className="code-editor glass">
      <div className="editor-toolbar">
        <span className="editor-title">
          {editorFile ? editorFile.split('/').pop() : 'Éditeur de code'}
        </span>
        <div className="editor-actions">
          <button onClick={pickFile}>Ouvrir</button>
          <button onClick={saveFile} disabled={!editorFile}>Sauvegarder</button>
        </div>
      </div>

      <div className="editor-body">
        {workspacePath && files.length > 0 && (
          <div className="file-tree">
            <p className="file-tree-label">Workspace</p>
            {files.map((f) => (
              <button
                key={f}
                className={`file-item ${editorFile.endsWith(f) ? 'active' : ''}`}
                onClick={() => openFile(f)}
              >
                📄 {f}
              </button>
            ))}
          </div>
        )}

        <div className="monaco-wrap">
          <Editor
            height="100%"
            language={editorLanguage}
            theme="vs-dark"
            value={editorContent}
            onChange={(v) => setEditorContent(v ?? '')}
            options={{
              fontSize: 14,
              fontFamily: 'JetBrains Mono, monospace',
              minimap: { enabled: true },
              scrollBeyondLastLine: false,
              padding: { top: 12 },
            }}
          />
        </div>
      </div>
    </div>
  );
}
