import { useStore } from '../stores/appStore';
import type { AppTab } from '../types';

const TABS: { id: AppTab; label: string; icon: string }[] = [
  { id: 'chat', label: 'Chat', icon: '💬' },
  { id: 'code', label: 'Code', icon: '⌨️' },
  { id: 'plugins', label: 'Plugins', icon: '🧩' },
];

export function TabBar() {
  const { activeTab, setActiveTab } = useStore();

  return (
    <nav className="tab-bar">
      {TABS.map((t) => (
        <button
          key={t.id}
          className={`tab-btn ${activeTab === t.id ? 'active' : ''}`}
          onClick={() => setActiveTab(t.id)}
        >
          <span>{t.icon}</span>
          <span>{t.label}</span>
        </button>
      ))}
    </nav>
  );
}
