import { useEffect, useRef } from 'react';
import { useStore } from '../stores/appStore';
import {
  initSpeechRecognition, startListening, stopListening,
  isSpeechSupported,
} from '../services/voice';

export function VoiceButton({ onTranscript }: { onTranscript: (text: string) => void }) {
  const { avatarState, setAvatarState, aiConfig } = useStore();
  const supported = isSpeechSupported();
  const onTranscriptRef = useRef(onTranscript);
  onTranscriptRef.current = onTranscript;

  useEffect(() => {
    if (!supported || !aiConfig.voiceEnabled) return;
    initSpeechRecognition(
      (text) => onTranscriptRef.current(text),
      (listening) => setAvatarState(listening ? 'listening' : 'idle'),
    );
  }, [supported, aiConfig.voiceEnabled, setAvatarState]);

  if (!supported || !aiConfig.voiceEnabled) return null;

  const isListening = avatarState === 'listening';

  return (
    <button
      className={`voice-btn ${isListening ? 'active' : ''}`}
      onClick={() => isListening ? stopListening() : startListening()}
      title={isListening ? 'Arrêter' : 'Parler'}
    >
      {isListening ? '🔴' : '🎤'}
    </button>
  );
}
