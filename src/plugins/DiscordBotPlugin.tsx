import { useState } from 'react';
import { useStore, getActiveProfile } from '../stores/appStore';
import { chat } from '../services/ai';

const BOT_TEMPLATE = `import discord
import os
from discord.ext import commands

intents = discord.Intents.default()
intents.message_content = True
bot = commands.Bot(command_prefix='!', intents=intents)

@bot.event
async def on_ready():
    print(f'{bot.user} connecté — Supremacy Bot')

@bot.command()
async def ping(ctx):
    await ctx.send(f'Pong! Latence: {round(bot.latency * 1000)}ms')

bot.run(os.getenv('DISCORD_TOKEN'))
`;

export function DiscordBotPlugin() {
  const [prompt, setPrompt] = useState('Bot de modération avec kick, ban, clear');
  const [code, setCode] = useState(BOT_TEMPLATE);
  const [loading, setLoading] = useState(false);
  const { aiConfig, workspacePath, setEditorFile } = useStore();

  const generate = async () => {
    setLoading(true);
    try {
      const profile = getActiveProfile();
      const response = await chat(
        [{ id: '1', role: 'user', content: `Génère un bot Discord Python complet : ${prompt}. Code production-ready avec discord.py.`, timestamp: Date.now() }],
        profile,
        aiConfig,
        workspacePath,
      );
      const match = response.match(/```(?:python)?\s*([\s\S]*?)```/);
      setCode(match ? match[1].trim() : response);
    } finally {
      setLoading(false);
    }
  };

  const saveToWorkspace = async () => {
    if (!workspacePath || !window.supremacy) return;
    const path = `${workspacePath}/supremacy_bot.py`;
    const allowed = await window.supremacy.askPermission('Créer bot Discord', path);
    if (!allowed) return;
    await window.supremacy.writeFile(path, code);
    setEditorFile(path, code, 'python');
  };

  return (
    <div className="plugin-content">
      <h3>🤖 Discord Bot Builder</h3>
      <input
        className="plugin-input"
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        placeholder="Décris ton bot..."
      />
      <div className="plugin-actions">
        <button onClick={generate} disabled={loading}>{loading ? 'Génération…' : 'Générer avec IA'}</button>
        <button onClick={saveToWorkspace}>Sauver dans workspace</button>
      </div>
      <pre className="plugin-code">{code}</pre>
    </div>
  );
}
