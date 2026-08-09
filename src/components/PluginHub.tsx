import { PLUGINS } from '../types';
import { useStore } from '../stores/appStore';
import { DiscordBotPlugin } from '../plugins/DiscordBotPlugin';
import { GameEnginePlugin } from '../plugins/GameEnginePlugin';
import { WebBuilderPlugin } from '../plugins/WebBuilderPlugin';
import { AwardFlightsPlugin } from '../plugins/AwardFlightsPlugin';

export function PluginHub() {
  const { activePlugin, setActivePlugin } = useStore();

  if (activePlugin) {
    return (
      <div className="plugin-hub glass">
        <div className="plugin-header">
          <button className="back-btn" onClick={() => setActivePlugin(null)}>← Plugins</button>
        </div>
        {activePlugin === 'discord-bot' && <DiscordBotPlugin />}
        {activePlugin === 'game-engine' && <GameEnginePlugin />}
        {activePlugin === 'web-builder' && <WebBuilderPlugin />}
        {activePlugin === 'award-flights' && <AwardFlightsPlugin />}
      </div>
    );
  }

  return (
    <div className="plugin-hub glass">
      <h2 className="plugin-hub-title">Plugins Supremacy</h2>
      <p className="plugin-hub-desc">Outils spécialisés propulsés par l'IA Core</p>
      <div className="plugin-grid">
        {PLUGINS.map((p) => (
          <button key={p.id} className="plugin-card" onClick={() => setActivePlugin(p.id)}>
            <span className="plugin-icon">{p.icon}</span>
            <span className="plugin-name">{p.name}</span>
            <span className="plugin-desc">{p.description}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
