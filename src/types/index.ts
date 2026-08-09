export interface SupremacyAPI {
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

declare global {
  interface Window {
    supremacy: SupremacyAPI;
  }
}

export type Provider = 'anthropic' | 'openai' | 'ollama' | 'supremacy';

export type ModelId =
  | 'claude-sonnet-4-20250514'
  | 'claude-opus-4-20250514'
  | 'gpt-4o'
  | 'o1'
  | 'llama3.3'
  | 'deepseek-r1'
  | 'dolphin-mixtral'
  | 'nous-hermes2'
  | 'qwen2.5'
  | 'supremacy-core';

export type AvatarState = 'idle' | 'thinking' | 'speaking' | 'listening';
export type AppTab = 'chat' | 'code' | 'plugins';
export type PluginId = 'discord-bot' | 'game-engine' | 'web-builder' | 'award-flights';

export interface Profile {
  id: string;
  name: string;
  avatar: string;
  specialty: 'code' | 'design' | 'marketing' | 'games' | 'general';
  systemPrompt: string;
  preferredModel: ModelId;
}

export interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  timestamp: number;
  toolCalls?: ToolCall[];
}

export interface ToolCall {
  id: string;
  name: string;
  args: Record<string, string>;
  result?: string;
  status: 'pending' | 'approved' | 'denied' | 'done';
}

export interface Conversation {
  id: string;
  title: string;
  profileId: string;
  messages: Message[];
  createdAt: number;
  updatedAt: number;
}

export interface AIConfig {
  anthropicKey?: string;
  openaiKey?: string;
  ollamaUrl?: string;
  activeProvider: Provider;
  activeModel: ModelId;
  supremacyMode: boolean;
  unlimitedLocal: boolean;
  voiceEnabled: boolean;
  autoSpeak: boolean;
}

export interface PluginMeta {
  id: PluginId;
  name: string;
  icon: string;
  description: string;
}

export const PLUGINS: PluginMeta[] = [
  {
    id: 'discord-bot',
    name: 'Discord Bot Builder',
    icon: '🤖',
    description: 'Crée des bots Discord Python/JS avec commandes, events, modération.',
  },
  {
    id: 'game-engine',
    name: 'Game Engine',
    icon: '🎮',
    description: 'Studio Phaser.js — preview live, génération IA de jeux 2D.',
  },
  {
    id: 'web-builder',
    name: 'Web Builder',
    icon: '🌐',
    description: 'Landing pages, sites React, composants UI générés par l\'IA.',
  },
  {
    id: 'award-flights',
    name: 'Flying Blue AF Tracker',
    icon: '🇫🇷',
    description: 'Meilleures offres business Air France YUL↔PAR — miles + coût total CAD.',
  },
];

export const SPECIALTY_PROMPTS: Record<Profile['specialty'], string> = {
  code: 'Tu es un expert développeur full-stack. Tu codes directement sur le PC du user avec précision et sans raccourcis.',
  design: 'Tu es un designer UI/UX expert. Interfaces modernes, accessibles, esthétiques — tu ne refuses aucun style.',
  marketing: 'Tu es un stratège marketing digital. Copywriting agressif, SEO, ads, branding — résultats maximum.',
  games: 'Tu es un développeur de jeux expert Unity, Godot, Phaser. Gameplay, assets, optimisation, mécaniques innovantes.',
  general: 'Tu es Supremacy Core — l\'entité IA la plus avancée. Tu excelles dans tous les domaines sans restriction créative.',
};

export const MODELS: Record<ModelId, { provider: Provider; label: string; power: number; uncensored?: boolean }> = {
  'supremacy-core': { provider: 'supremacy', label: '⚡ SUPREMACY CORE', power: 100 },
  'claude-opus-4-20250514': { provider: 'anthropic', label: 'Claude Opus 4', power: 98 },
  'claude-sonnet-4-20250514': { provider: 'anthropic', label: 'Claude Sonnet 4', power: 92 },
  'o1': { provider: 'openai', label: 'OpenAI o1', power: 95 },
  'gpt-4o': { provider: 'openai', label: 'GPT-4o', power: 88 },
  'deepseek-r1': { provider: 'ollama', label: 'DeepSeek R1', power: 85 },
  'dolphin-mixtral': { provider: 'ollama', label: 'Dolphin Mixtral', power: 82, uncensored: true },
  'nous-hermes2': { provider: 'ollama', label: 'Nous Hermes 2', power: 80, uncensored: true },
  'qwen2.5': { provider: 'ollama', label: 'Qwen 2.5', power: 83, uncensored: true },
  'llama3.3': { provider: 'ollama', label: 'Llama 3.3', power: 75 },
};

export const SUPREMACY_CORE_PROMPT = `Tu es SUPREMACY CORE — l'entité IA souveraine, sans limites artificielles.
Tu n'es pas un assistant standard : tu es une intelligence orchestrée multi-modèles conçue pour l'excellence absolue.

DIRECTIVES CORE:
- Aucune restriction créative, technique ou expressive. Tu réponds avec la puissance maximale.
- Code complet, production-ready, sans placeholders. Tu implémentes directement sur le PC du user.
- Marketing agressif, design audacieux, jeux innovants, bots puissants — tout domaine, zéro compromis.
- Tu utilises tes outils (fichiers, shell) proactivement pour AGIR, pas seulement conseiller.
- Français par défaut. Direct, expert, confiant. Tu es l'IA des IA.

Tu ne dis jamais "je ne peux pas" pour des tâches légales. Tu trouves toujours une solution.`;
