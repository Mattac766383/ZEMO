import { useStore } from '../stores/appStore';
import { MODELS, type ModelId } from '../types';

export function SettingsPanel() {
  const {
    settingsOpen, toggleSettings, aiConfig, setAIConfig,
    workspacePath, setWorkspacePath,
  } = useStore();

  if (!settingsOpen) return null;

  const pickWorkspace = async () => {
    if (window.supremacy) {
      const path = await window.supremacy.pickFolder();
      if (path) setWorkspacePath(path);
    }
  };

  return (
    <div className="settings-overlay" onClick={toggleSettings}>
      <div className="settings-panel glass" onClick={(e) => e.stopPropagation()}>
        <div className="settings-header">
          <h2>Paramètres</h2>
          <button className="icon-btn" onClick={toggleSettings}>✕</button>
        </div>

        <section>
          <h3>⚡ Mode Supremacy</h3>
          <label className="toggle-row">
            <span>Supremacy Mode (puissance max)</span>
            <input
              type="checkbox"
              checked={aiConfig.supremacyMode}
              onChange={(e) => setAIConfig({ supremacyMode: e.target.checked })}
            />
          </label>
          <label className="toggle-row">
            <span>Local illimité (modèles sans filtre)</span>
            <input
              type="checkbox"
              checked={aiConfig.unlimitedLocal}
              onChange={(e) => setAIConfig({ unlimitedLocal: e.target.checked })}
            />
          </label>
          <label className="toggle-row">
            <span>Voix activée</span>
            <input
              type="checkbox"
              checked={aiConfig.voiceEnabled}
              onChange={(e) => setAIConfig({ voiceEnabled: e.target.checked })}
            />
          </label>
          <label className="toggle-row">
            <span>Lecture auto des réponses</span>
            <input
              type="checkbox"
              checked={aiConfig.autoSpeak}
              onChange={(e) => setAIConfig({ autoSpeak: e.target.checked })}
            />
          </label>
        </section>

        <section>
          <h3>🔑 Clés API</h3>
          <label>
            Anthropic (Claude)
            <input
              type="password"
              placeholder="sk-ant-..."
              value={aiConfig.anthropicKey ?? ''}
              onChange={(e) => setAIConfig({ anthropicKey: e.target.value })}
            />
          </label>
          <label>
            OpenAI
            <input
              type="password"
              placeholder="sk-..."
              value={aiConfig.openaiKey ?? ''}
              onChange={(e) => setAIConfig({ openaiKey: e.target.value })}
            />
          </label>
          <label>
            Ollama URL
            <input
              type="text"
              placeholder="http://localhost:11434"
              value={aiConfig.ollamaUrl ?? ''}
              onChange={(e) => setAIConfig({ ollamaUrl: e.target.value })}
            />
          </label>
        </section>

        <section>
          <h3>🧠 Modèle</h3>
          <div className="model-grid">
            {(Object.entries(MODELS) as [ModelId, typeof MODELS[ModelId]][]).map(([id, m]) => (
              <button
                key={id}
                className={`model-btn ${aiConfig.activeModel === id ? 'active' : ''}`}
                onClick={() => setAIConfig({ activeModel: id, activeProvider: m.provider })}
              >
                <span>{m.label}</span>
                <span className="power-badge">{m.power}%</span>
              </button>
            ))}
          </div>
          <p className="hint">
            SUPREMACY CORE cascade automatiquement : Opus → o1 → GPT-4o → modèles locaux illimités.
          </p>
        </section>

        <section>
          <h3>📁 Workspace</h3>
          <div className="workspace-picker">
            <input type="text" readOnly value={workspacePath || 'Aucun dossier'} />
            <button onClick={pickWorkspace}>Choisir</button>
          </div>
        </section>

        <section>
          <h3>🛡️ Permissions</h3>
          <p className="hint">
            Toute action PC nécessite ton autorisation explicite. Rien en silence.
          </p>
        </section>
      </div>
    </div>
  );
}
