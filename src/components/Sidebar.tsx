import { motion } from 'framer-motion';
import { useStore } from '../stores/appStore';

export function Sidebar() {
  const {
    profiles, activeProfileId, setActiveProfile, sidebarOpen, toggleSidebar,
    conversations, activeConversationId, selectConversation, deleteConversation, newConversation,
  } = useStore();

  if (!sidebarOpen) {
    return (
      <button className="sidebar-toggle collapsed" onClick={toggleSidebar} title="Ouvrir">☰</button>
    );
  }

  return (
    <motion.aside
      className="sidebar glass"
      initial={{ x: -280 }}
      animate={{ x: 0 }}
      transition={{ type: 'spring', stiffness: 300, damping: 30 }}
    >
      <div className="sidebar-header">
        <div className="logo">
          <span className="logo-icon">⚡</span>
          <span className="logo-text">SUPREMACY</span>
        </div>
        <button className="icon-btn" onClick={toggleSidebar}>✕</button>
      </div>

      <p className="sidebar-label">Profils IA</p>
      <div className="profile-list">
        {profiles.map((p) => (
          <button
            key={p.id}
            className={`profile-card ${p.id === activeProfileId ? 'active' : ''}`}
            onClick={() => setActiveProfile(p.id)}
          >
            <span className="profile-avatar">{p.avatar}</span>
            <div className="profile-info">
              <span className="profile-name">{p.name}</span>
              <span className="profile-specialty">{p.specialty}</span>
            </div>
            {p.id === activeProfileId && <span className="active-dot" />}
          </button>
        ))}
      </div>

      <div className="conv-section">
        <div className="conv-header">
          <p className="sidebar-label">Historique</p>
          <button className="icon-btn" onClick={newConversation}>+</button>
        </div>
        <div className="conv-list">
          {conversations.map((c) => (
            <div
              key={c.id}
              className={`conv-item ${c.id === activeConversationId ? 'active' : ''}`}
              onClick={() => selectConversation(c.id)}
            >
              <span className="conv-title">{c.title}</span>
              <button
                className="conv-delete"
                onClick={(e) => { e.stopPropagation(); deleteConversation(c.id); }}
              >×</button>
            </div>
          ))}
        </div>
      </div>

      <div className="sidebar-footer">
        <p className="power-label">SUPREMACY CORE — 100%</p>
        <div className="power-bar">
          <div className="power-fill" />
        </div>
      </div>
    </motion.aside>
  );
}
