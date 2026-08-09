import { useState } from 'react';
import { useStore, getActiveProfile } from '../stores/appStore';
import { chat } from '../services/ai';

export function WebBuilderPlugin() {
  const [prompt, setPrompt] = useState('Landing page SaaS IA avec hero animé');
  const [html, setHtml] = useState('');
  const [loading, setLoading] = useState(false);
  const { aiConfig, workspacePath, setEditorFile } = useStore();

  const generate = async () => {
    setLoading(true);
    try {
      const profile = getActiveProfile();
      const response = await chat(
        [{ id: '1', role: 'user', content: `Génère une page HTML/CSS complète : ${prompt}. Style dark, violet/bleu glass, moderne, responsive. Un seul fichier HTML avec CSS inline.`, timestamp: Date.now() }],
        profile,
        aiConfig,
        workspacePath,
      );
      const match = response.match(/```(?:html)?\s*([\s\S]*?)```/);
      setHtml(match ? match[1].trim() : response);
    } finally {
      setLoading(false);
    }
  };

  const save = async () => {
    if (!workspacePath || !window.supremacy || !html) return;
    const path = `${workspacePath}/supremacy_page.html`;
    const allowed = await window.supremacy.askPermission('Créer page web', path);
    if (!allowed) return;
    await window.supremacy.writeFile(path, html);
    setEditorFile(path, html, 'html');
  };

  return (
    <div className="plugin-content">
      <h3>🌐 Web Builder</h3>
      <input className="plugin-input" value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="Décris ta page..." />
      <div className="plugin-actions">
        <button onClick={generate} disabled={loading}>{loading ? 'Génération…' : 'Générer page IA'}</button>
        <button onClick={save} disabled={!html}>Sauver workspace</button>
      </div>
      {html && (
        <div className="web-preview">
          <iframe srcDoc={html} title="Web Preview" sandbox="allow-scripts" />
        </div>
      )}
    </div>
  );
}
