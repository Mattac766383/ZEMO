import { Scene3D } from './components/Scene3D';
import { Sidebar } from './components/Sidebar';
import { ChatPanel } from './components/ChatPanel';
import { SettingsPanel } from './components/SettingsPanel';
import { CodeEditor } from './components/CodeEditor';
import { PluginHub } from './components/PluginHub';
import { TabBar } from './components/TabBar';
import { useStore } from './stores/appStore';
import { useEffect } from 'react';
import { loadPersistedState } from './services/persistence';

export function App() {
  const { activeTab, toggleSettings, hydrate } = useStore();

  useEffect(() => {
    loadPersistedState().then((data) => {
      if (data) hydrate(data as Parameters<typeof hydrate>[0]);
    });
    if (window.supremacy) {
      window.supremacy.getHome().then((home) => {
        if (!useStore.getState().workspacePath) {
          useStore.getState().setWorkspacePath(home);
        }
      });
    }
  }, [hydrate]);

  return (
    <div className="app">
      <Scene3D />
      <div className="ui-layer">
        <Sidebar />
        <main className="main-content">
          <header className="top-bar">
            <TabBar />
            <button className="settings-btn" onClick={toggleSettings}>⚙️</button>
          </header>
          <div className="content-area">
            {activeTab === 'chat' && <ChatPanel />}
            {activeTab === 'code' && <CodeEditor />}
            {activeTab === 'plugins' && <PluginHub />}
          </div>
        </main>
      </div>
      <SettingsPanel />
    </div>
  );
}
