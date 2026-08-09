# Supremacy

Assistant IA souverain avec interface 3D — code, design, marketing, jeux, bots.

## Fonctionnalités

- **⚡ SUPREMACY CORE** — orchestration multi-modèles en cascade (Opus → o1 → GPT-4o → local)
- **Interface 3D** — avatar réactif (idle, thinking, speaking, listening), orbes violets, verre bleu
- **5 profils IA** — Supremacy, Architect, Pixel, Growth, Nexus
- **Voix** — speech-to-text (🎤) + lecture auto des réponses
- **Éditeur Monaco** — codage direct sur le PC avec file tree
- **Historique persistant** — conversations sauvegardées automatiquement
- **Plugins** — Discord Bot Builder, Game Engine (Phaser), Web Builder
- **Contrôle PC** — fichiers + shell avec autorisation explicite

## Lancer

```bash
cd ~/supremacy
npm run electron:dev
```

## Configuration

1. **⚙️ Paramètres** → clé Anthropic (recommandé) ou OpenAI
2. Activer **Supremacy Mode** + **Local illimité** pour puissance max
3. Choisir un **workspace** pour le codage direct
4. (Optionnel) Ollama pour modèles locaux sans filtre :
   ```bash
   ollama pull dolphin-mixtral
   ollama pull nous-hermes2
   ollama pull qwen2.5
   ```

## Modèles

| Modèle | Type | Puissance |
|--------|------|-----------|
| SUPREMACY CORE | Cascade auto | 100% |
| Claude Opus 4 | API | 98% |
| OpenAI o1 | API | 95% |
| GPT-4o | API | 88% |
| Dolphin Mixtral | Local illimité | 82% |
| Nous Hermes 2 | Local illimité | 80% |

## Build app

```bash
npm run electron:build
```

## Sécurité

Toute action sur le PC nécessite une popup d'autorisation native.
