let recognition: SpeechRecognition | null = null;

function getSpeechRecognitionCtor(): SpeechRecognitionConstructor | null {
  return window.SpeechRecognition ?? window.webkitSpeechRecognition ?? null;
}

export function initSpeechRecognition(onResult: (text: string) => void, onState: (listening: boolean) => void): boolean {
  const SR = getSpeechRecognitionCtor();
  if (!SR) return false;

  const rec = new SR();
  recognition = rec;
  rec.lang = 'fr-FR';
  rec.continuous = false;
  rec.interimResults = true;

  rec.onstart = () => onState(true);
  rec.onend = () => onState(false);
  rec.onerror = () => onState(false);

  rec.onresult = (event) => {
    const text = Array.from(event.results)
      .map((r) => r[0].transcript)
      .join('');
    if (event.results[event.results.length - 1].isFinal) {
      onResult(text);
    }
  };

  return true;
}

export function startListening() {
  recognition?.start();
}

export function stopListening() {
  recognition?.stop();
}

export function speak(text: string, onEnd?: () => void) {
  window.speechSynthesis.cancel();
  const utterance = new SpeechSynthesisUtterance(text.slice(0, 2000));
  utterance.lang = 'fr-FR';
  utterance.rate = 1.05;
  utterance.onend = () => onEnd?.();
  window.speechSynthesis.speak(utterance);
}

export function stopSpeaking() {
  window.speechSynthesis.cancel();
}

export function isSpeechSupported(): boolean {
  return !!getSpeechRecognitionCtor();
}
