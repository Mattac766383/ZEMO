import { useRef, useEffect } from 'react';
import { motion } from 'framer-motion';
import { useStore, getActiveProfile, getActiveMessages } from '../stores/appStore';
import { chat } from '../services/ai';
import { speak, stopSpeaking } from '../services/voice';
import { VoiceButton } from './VoiceButton';

export function ChatPanel() {
  const {
    addMessage, isThinking, setThinking, aiConfig, workspacePath,
    setAvatarState,
  } = useStore();
  const messages = getActiveMessages();
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, isThinking]);

  const sendMessage = async (text: string) => {
    if (!text.trim() || isThinking) return;

    addMessage({ role: 'user', content: text });
    setThinking(true);

    try {
      const profile = getActiveProfile();
      const allMessages = [...getActiveMessages()];
      const response = await chat(allMessages, profile, aiConfig, workspacePath);
      addMessage({ role: 'assistant', content: response });

      if (aiConfig.autoSpeak && aiConfig.voiceEnabled) {
        setAvatarState('speaking');
        speak(response, () => setAvatarState('idle'));
      }
    } catch (e) {
      addMessage({ role: 'assistant', content: `Erreur : ${String(e)}` });
    } finally {
      setThinking(false);
    }
  };

  const send = () => {
    const text = inputRef.current?.value?.trim();
    if (!text) return;
    inputRef.current!.value = '';
    sendMessage(text);
  };

  const profile = getActiveProfile();

  return (
    <div className="chat-panel glass">
      <div className="chat-header">
        <div className="chat-profile">
          <span className="chat-avatar">{profile.avatar}</span>
          <div>
            <h2>{profile.name}</h2>
            <span className="chat-model">
              {aiConfig.activeModel === 'supremacy-core' ? '⚡ SUPREMACY CORE' : aiConfig.activeModel}
              {aiConfig.supremacyMode && ' · MODE MAX'}
            </span>
          </div>
        </div>
        <div className="chat-header-actions">
          {aiConfig.voiceEnabled && (
            <button className="icon-btn" onClick={() => stopSpeaking()} title="Stop voix">🔇</button>
          )}
          <button className="icon-btn" onClick={() => useStore.getState().newConversation()}>
            + Nouveau
          </button>
        </div>
      </div>

      <div className="chat-messages">
        {messages.length === 0 && (
          <motion.div className="welcome" initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }}>
            <h3>SUPREMACY CORE</h3>
            <p>L'intelligence sans limites. Code, design, marketing, jeux, bots — tout, sans compromis.</p>
            <div className="quick-prompts">
              {['Crée un SaaS complet', 'Bot Discord modération', 'Jeu Phaser spatial', 'Landing page agressive'].map((q) => (
                <button key={q} className="quick-btn" onClick={() => sendMessage(q)}>{q}</button>
              ))}
            </div>
          </motion.div>
        )}

        {messages.map((m) => (
          <motion.div
            key={m.id}
            className={`message ${m.role}`}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
          >
            <div className="message-bubble">{m.content}</div>
          </motion.div>
        ))}

        {isThinking && (
          <div className="message assistant thinking">
            <div className="message-bubble">
              <span className="dot-pulse" />
              <span className="dot-pulse" />
              <span className="dot-pulse" />
            </div>
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      <div className="chat-input-area">
        <VoiceButton onTranscript={(text) => {
          if (inputRef.current) inputRef.current.value = text;
          sendMessage(text);
        }} />
        <textarea
          ref={inputRef}
          className="chat-input"
          placeholder="Commande Supremacy..."
          rows={2}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <button className="send-btn" onClick={send} disabled={isThinking}>
          {isThinking ? '◌' : '▶'}
        </button>
      </div>
    </div>
  );
}
