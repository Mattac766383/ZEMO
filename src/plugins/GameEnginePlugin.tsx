import { useState, useRef, useEffect } from 'react';
import { useStore, getActiveProfile } from '../stores/appStore';
import { chat } from '../services/ai';

const DEFAULT_GAME = `// Phaser 3 — Jeu généré par Supremacy
const config = {
  type: Phaser.AUTO, width: 480, height: 320,
  physics: { default: 'arcade', arcade: { gravity: { y: 300 } } },
  scene: { preload, create, update }
};
let player, cursors, score = 0, scoreText;

function preload() {
  this.load.image('sky', 'https://labs.phaser.io/assets/skies/space3.png');
  this.load.spritesheet('dude', 'https://labs.phaser.io/assets/sprites/dude.png', { frameWidth: 32, frameHeight: 48 });
}

function create() {
  this.add.image(240, 160, 'sky');
  const platforms = this.physics.add.staticGroup();
  platforms.create(240, 300, null).setScale(2).refreshBody();
  player = this.physics.add.sprite(100, 200, 'dude');
  player.setBounce(0.2).setCollideWorldBounds(true);
  this.physics.add.collider(player, platforms);
  cursors = this.input.keyboard.createCursorKeys();
  scoreText = this.add.text(16, 16, 'Score: 0', { fontSize: '18px', fill: '#9b59f5' });
}

function update() {
  if (cursors.left.isDown) player.setVelocityX(-160);
  else if (cursors.right.isDown) player.setVelocityX(160);
  else player.setVelocityX(0);
  if (cursors.up.isDown && player.body.touching.down) player.setVelocityY(-330);
}

new Phaser.Game(config);
`;

export function GameEnginePlugin() {
  const [prompt, setPrompt] = useState('Plateformer spatial avec score');
  const [gameCode, setGameCode] = useState(DEFAULT_GAME);
  const [loading, setLoading] = useState(false);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const { aiConfig, workspacePath } = useStore();

  const runPreview = () => {
    const html = `<!DOCTYPE html><html><head>
      <script src="https://cdn.jsdelivr.net/npm/phaser@3.80.1/dist/phaser.min.js"></script>
      <style>body{margin:0;background:#050510}</style>
    </head><body><script>${gameCode}</script></body></html>`;
    if (iframeRef.current) iframeRef.current.srcdoc = html;
  };

  useEffect(() => { runPreview(); }, []);

  const generate = async () => {
    setLoading(true);
    try {
      const profile = getActiveProfile();
      const response = await chat(
        [{ id: '1', role: 'user', content: `Génère un jeu Phaser 3 complet : ${prompt}. Code JS autonome, new Phaser.Game à la fin.`, timestamp: Date.now() }],
        profile,
        aiConfig,
        workspacePath,
      );
      const match = response.match(/```(?:javascript|js)?\s*([\s\S]*?)```/);
      const code = match ? match[1].trim() : response;
      setGameCode(code);
      setTimeout(runPreview, 100);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="plugin-content">
      <h3>🎮 Game Engine — Phaser.js</h3>
      <input className="plugin-input" value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="Décris ton jeu..." />
      <div className="plugin-actions">
        <button onClick={generate} disabled={loading}>{loading ? 'Génération…' : 'Générer jeu IA'}</button>
        <button onClick={runPreview}>▶ Preview</button>
      </div>
      <div className="game-preview">
        <iframe ref={iframeRef} title="Game Preview" sandbox="allow-scripts" />
      </div>
      <pre className="plugin-code">{gameCode.slice(0, 500)}…</pre>
    </div>
  );
}
