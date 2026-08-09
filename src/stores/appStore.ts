import { create } from 'zustand';
import { v4 as uuid } from 'uuid';
import type {
  Profile, Message, AIConfig, Conversation, AvatarState, AppTab, PluginId,
} from '../types';
import { SPECIALTY_PROMPTS } from '../types';
import { savePersistedState, deriveTitle } from '../services/persistence';

const DEFAULT_PROFILES: Profile[] = [
  {
    id: 'profile-supremacy',
    name: 'Supremacy',
    avatar: '⚡',
    specialty: 'general',
    systemPrompt: SPECIALTY_PROMPTS.general,
    preferredModel: 'supremacy-core',
  },
  {
    id: 'profile-architect',
    name: 'Architect',
    avatar: '🏗️',
    specialty: 'code',
    systemPrompt: SPECIALTY_PROMPTS.code,
    preferredModel: 'claude-sonnet-4-20250514',
  },
  {
    id: 'profile-pixel',
    name: 'Pixel',
    avatar: '🎨',
    specialty: 'design',
    systemPrompt: SPECIALTY_PROMPTS.design,
    preferredModel: 'gpt-4o',
  },
  {
    id: 'profile-growth',
    name: 'Growth',
    avatar: '📈',
    specialty: 'marketing',
    systemPrompt: SPECIALTY_PROMPTS.marketing,
    preferredModel: 'gpt-4o',
  },
  {
    id: 'profile-nexus',
    name: 'Nexus',
    avatar: '🎮',
    specialty: 'games',
    systemPrompt: SPECIALTY_PROMPTS.games,
    preferredModel: 'claude-sonnet-4-20250514',
  },
];

function newConversation(profileId: string): Conversation {
  return {
    id: uuid(),
    title: 'Nouvelle conversation',
    profileId,
    messages: [],
    createdAt: Date.now(),
    updatedAt: Date.now(),
  };
}

interface AppState {
  profiles: Profile[];
  activeProfileId: string;
  conversations: Conversation[];
  activeConversationId: string;
  aiConfig: AIConfig;
  workspacePath: string;
  isThinking: boolean;
  sidebarOpen: boolean;
  settingsOpen: boolean;
  activeTab: AppTab;
  activePlugin: PluginId | null;
  avatarState: AvatarState;
  editorFile: string;
  editorContent: string;
  editorLanguage: string;

  hydrate: (data: Partial<AppState>) => void;
  persist: () => void;
  setActiveProfile: (id: string) => void;
  setActiveTab: (tab: AppTab) => void;
  setActivePlugin: (id: PluginId | null) => void;
  setAvatarState: (state: AvatarState) => void;
  addMessage: (msg: Omit<Message, 'id' | 'timestamp'>) => void;
  setAIConfig: (patch: Partial<AIConfig>) => void;
  setWorkspacePath: (path: string) => void;
  setThinking: (v: boolean) => void;
  toggleSidebar: () => void;
  toggleSettings: () => void;
  newConversation: () => void;
  selectConversation: (id: string) => void;
  deleteConversation: (id: string) => void;
  setEditorFile: (path: string, content: string, language?: string) => void;
  setEditorContent: (content: string) => void;
}

export const useStore = create<AppState>((set, get) => ({
  profiles: DEFAULT_PROFILES,
  activeProfileId: DEFAULT_PROFILES[0].id,
  conversations: [newConversation(DEFAULT_PROFILES[0].id)],
  activeConversationId: '',
  aiConfig: {
    activeProvider: 'supremacy',
    activeModel: 'supremacy-core',
    ollamaUrl: 'http://localhost:11434',
    supremacyMode: true,
    unlimitedLocal: true,
    voiceEnabled: true,
    autoSpeak: false,
  },
  workspacePath: '',
  isThinking: false,
  sidebarOpen: true,
  settingsOpen: false,
  activeTab: 'chat',
  activePlugin: null,
  avatarState: 'idle',
  editorFile: '',
  editorContent: '',
  editorLanguage: 'typescript',

  hydrate: (data) => {
    const conv = data.conversations?.[0];
    set({
      ...data,
      activeConversationId: data.activeConversationId ?? conv?.id ?? get().activeConversationId,
    });
  },

  persist: () => {
    const s = get();
    savePersistedState({
      conversations: s.conversations,
      activeConversationId: s.activeConversationId,
      aiConfig: s.aiConfig,
      workspacePath: s.workspacePath,
      profiles: s.profiles,
      activeProfileId: s.activeProfileId,
    });
  },

  setActiveProfile: (id) => {
    set({ activeProfileId: id });
    get().persist();
  },

  setActiveTab: (tab) => set({ activeTab: tab }),
  setActivePlugin: (id) => set({ activePlugin: id, activeTab: 'plugins' }),
  setAvatarState: (state) => set({ avatarState: state }),

  addMessage: (msg) => {
    const message: Message = { ...msg, id: uuid(), timestamp: Date.now() };
    set((s) => {
      const convId = s.activeConversationId || s.conversations[0]?.id;
      const conversations = s.conversations.map((c) => {
        if (c.id !== convId) return c;
        const messages = [...c.messages, message];
        return {
          ...c,
          messages,
          title: c.messages.length === 0 ? deriveTitle(messages) : c.title,
          updatedAt: Date.now(),
        };
      });
      return { conversations };
    });
    get().persist();
  },

  setAIConfig: (patch) => {
    set((s) => ({ aiConfig: { ...s.aiConfig, ...patch } }));
    get().persist();
  },

  setWorkspacePath: (path) => {
    set({ workspacePath: path });
    get().persist();
  },

  setThinking: (v) => set({ isThinking: v, avatarState: v ? 'thinking' : 'idle' }),

  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
  toggleSettings: () => set((s) => ({ settingsOpen: !s.settingsOpen })),

  newConversation: () => {
    const conv = newConversation(get().activeProfileId);
    set((s) => ({
      conversations: [conv, ...s.conversations],
      activeConversationId: conv.id,
    }));
    get().persist();
  },

  selectConversation: (id) => {
    set({ activeConversationId: id });
    get().persist();
  },

  deleteConversation: (id) => {
    set((s) => {
      const conversations = s.conversations.filter((c) => c.id !== id);
      if (conversations.length === 0) {
        const conv = newConversation(s.activeProfileId);
        return { conversations: [conv], activeConversationId: conv.id };
      }
      const activeConversationId =
        s.activeConversationId === id ? conversations[0].id : s.activeConversationId;
      return { conversations, activeConversationId };
    });
    get().persist();
  },

  setEditorFile: (path, content, language = 'typescript') =>
    set({ editorFile: path, editorContent: content, editorLanguage: language, activeTab: 'code' }),

  setEditorContent: (content) => set({ editorContent: content }),
}));

// Init active conversation id
const initial = useStore.getState();
if (!initial.activeConversationId && initial.conversations[0]) {
  useStore.setState({ activeConversationId: initial.conversations[0].id });
}

export function getActiveProfile(): Profile {
  const { profiles, activeProfileId } = useStore.getState();
  return profiles.find((p) => p.id === activeProfileId) ?? profiles[0];
}

export function getActiveMessages(): Message[] {
  const { conversations, activeConversationId } = useStore.getState();
  const conv = conversations.find((c) => c.id === activeConversationId);
  return conv?.messages ?? [];
}
