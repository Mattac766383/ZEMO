import type { Conversation, AIConfig, Profile } from '../types';

interface PersistedState {
  conversations: Conversation[];
  activeConversationId: string | null;
  aiConfig: AIConfig;
  workspacePath: string;
  profiles: Profile[];
  activeProfileId: string;
}

export async function loadPersistedState(): Promise<Partial<PersistedState> | null> {
  if (!window.supremacy?.storageLoad) return null;
  const data = await window.supremacy.storageLoad();
  return data as Partial<PersistedState> | null;
}

export async function savePersistedState(state: PersistedState): Promise<void> {
  if (!window.supremacy?.storageSave) return;
  await window.supremacy.storageSave(state as unknown as Record<string, unknown>);
}

export function deriveTitle(messages: { content: string }[]): string {
  const first = messages.find((m) => m.content.trim());
  if (!first) return 'Nouvelle conversation';
  return first.content.slice(0, 40) + (first.content.length > 40 ? '…' : '');
}
