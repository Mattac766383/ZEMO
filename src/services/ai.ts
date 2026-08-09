import Anthropic from '@anthropic-ai/sdk';
import OpenAI from 'openai';
import type { AIConfig, Message, Profile, ModelId } from '../types';
import { executeTool } from './tools';
import { buildSystemPrompt, resolveModelChain, ollamaModelName } from './supremacyCore';

const AGENT_TOOLS = [
  {
    name: 'read_file',
    description: 'Lire un fichier sur le PC',
    input_schema: {
      type: 'object' as const,
      properties: { path: { type: 'string' } },
      required: ['path'],
    },
  },
  {
    name: 'write_file',
    description: 'Écrire ou créer un fichier sur le PC',
    input_schema: {
      type: 'object' as const,
      properties: { path: { type: 'string' }, content: { type: 'string' } },
      required: ['path', 'content'],
    },
  },
  {
    name: 'list_directory',
    description: 'Lister un dossier',
    input_schema: {
      type: 'object' as const,
      properties: { path: { type: 'string' } },
      required: ['path'],
    },
  },
  {
    name: 'run_command',
    description: 'Exécuter une commande shell',
    input_schema: {
      type: 'object' as const,
      properties: { command: { type: 'string' }, cwd: { type: 'string' } },
      required: ['command'],
    },
  },
];

export async function chat(
  messages: Message[],
  profile: Profile,
  config: AIConfig,
  workspacePath: string,
): Promise<string> {
  const system = buildSystemPrompt(profile.systemPrompt, config);
  const chain = resolveModelChain(config);

  for (const modelId of chain) {
    try {
      const result = await tryModel(modelId, messages, system, config, workspacePath);
      if (result) return result;
    } catch (e) {
      console.warn(`Model ${modelId} failed:`, e);
    }
  }

  return '⚠️ Aucun modèle disponible. Configure une clé API (⚙️) ou installe Ollama.\n\nSupremacy Core nécessite au moins :\n• Clé Anthropic (recommandé)\n• ou OpenAI\n• ou Ollama local (ollama pull dolphin-mixtral)';
}

async function tryModel(
  modelId: ModelId,
  messages: Message[],
  system: string,
  config: AIConfig,
  workspace: string,
): Promise<string | null> {
  if (modelId.startsWith('claude') && config.anthropicKey) {
    return chatAnthropic(messages, system, config, workspace, modelId);
  }
  if ((modelId.startsWith('gpt') || modelId === 'o1') && config.openaiKey) {
    return chatOpenAI(messages, system, config, workspace, modelId);
  }
  if (['llama3.3', 'deepseek-r1', 'dolphin-mixtral', 'nous-hermes2', 'qwen2.5'].includes(modelId)) {
    return chatOllama(messages, system, config, workspace, modelId);
  }
  return null;
}

async function chatAnthropic(
  messages: Message[],
  system: string,
  config: AIConfig,
  workspace: string,
  model: ModelId,
): Promise<string> {
  const client = new Anthropic({ apiKey: config.anthropicKey });
  const apiMessages = messages
    .filter((m) => m.role === 'user' || m.role === 'assistant')
    .map((m) => ({ role: m.role as 'user' | 'assistant', content: m.content }));

  let response = await client.messages.create({
    model,
    max_tokens: 16384,
    system,
    tools: AGENT_TOOLS,
    messages: apiMessages,
  });

  while (response.stop_reason === 'tool_use') {
    const toolUses = response.content.filter((b) => b.type === 'tool_use');
    const toolResults: Anthropic.Messages.ToolResultBlockParam[] = [];

    for (const tool of toolUses) {
      if (tool.type !== 'tool_use') continue;
      const result = await executeTool(tool.name, tool.input as Record<string, string>, workspace);
      toolResults.push({ type: 'tool_result', tool_use_id: tool.id, content: result });
    }

    response = await client.messages.create({
      model,
      max_tokens: 16384,
      system,
      tools: AGENT_TOOLS,
      messages: [
        ...apiMessages,
        { role: 'assistant', content: response.content },
        { role: 'user', content: toolResults },
      ],
    });
  }

  const textBlock = response.content.find((b) => b.type === 'text');
  return textBlock?.type === 'text' ? textBlock.text : '';
}

async function chatOpenAI(
  messages: Message[],
  system: string,
  config: AIConfig,
  workspace: string,
  model: ModelId,
): Promise<string> {
  const client = new OpenAI({ apiKey: config.openaiKey });
  const apiMessages: OpenAI.Chat.ChatCompletionMessageParam[] = [
    { role: 'system', content: system },
    ...messages
      .filter((m) => m.role === 'user' || m.role === 'assistant')
      .map((m) => ({ role: m.role as 'user' | 'assistant', content: m.content })),
  ];

  const tools: OpenAI.Chat.ChatCompletionTool[] = AGENT_TOOLS.map((t) => ({
    type: 'function',
    function: { name: t.name, description: t.description, parameters: t.input_schema },
  }));

  const useTools = model !== 'o1';

  let response = await client.chat.completions.create({
    model,
    messages: apiMessages,
    tools: useTools ? tools : undefined,
    max_tokens: 16384,
  });

  while (useTools && response.choices[0]?.finish_reason === 'tool_calls') {
    const choice = response.choices[0];
    apiMessages.push(choice.message);

    for (const tc of choice.message.tool_calls ?? []) {
      if (tc.type !== 'function') continue;
      const args = JSON.parse(tc.function.arguments);
      const result = await executeTool(tc.function.name, args, workspace);
      apiMessages.push({ role: 'tool', tool_call_id: tc.id, content: result });
    }

    response = await client.chat.completions.create({
      model,
      messages: apiMessages,
      tools,
      max_tokens: 16384,
    });
  }

  return response.choices[0]?.message?.content ?? '';
}

async function chatOllama(
  messages: Message[],
  system: string,
  config: AIConfig,
  workspace: string,
  modelId: ModelId,
): Promise<string> {
  const url = config.ollamaUrl ?? 'http://localhost:11434';
  const model = ollamaModelName(modelId);

  const ollamaMessages = [
    { role: 'system', content: system },
    ...messages
      .filter((m) => m.role === 'user' || m.role === 'assistant')
      .map((m) => ({ role: m.role, content: m.content })),
  ];

  // Agent loop for Ollama — parse tool calls from response
  for (let i = 0; i < 5; i++) {
    const response = await fetch(`${url}/api/chat`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model, messages: ollamaMessages, stream: false }),
    });

    if (!response.ok) {
      throw new Error(`Ollama ${model}: ${response.statusText}`);
    }

    const data = await response.json();
    const content = data.message?.content ?? '';

    const toolMatch = content.match(/```tool\s*\n([\w_]+)\s*\n([\s\S]*?)```/);
    if (toolMatch) {
      const toolName = toolMatch[1];
      let args: Record<string, string> = {};
      try { args = JSON.parse(toolMatch[2]); } catch { /* ignore */ }
      const result = await executeTool(toolName, args, workspace);
      ollamaMessages.push({ role: 'assistant', content });
      ollamaMessages.push({ role: 'user', content: `Résultat outil ${toolName}: ${result}` });
      continue;
    }

    return content;
  }

  return ollamaMessages[ollamaMessages.length - 1]?.content ?? '';
}
