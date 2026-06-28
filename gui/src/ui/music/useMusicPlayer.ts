// Adapted from an internal effects library.
// ========== MUSIC PLAYER LOGIC HOOK ==========

import { useCallback, useEffect, useRef, useState } from 'react';

export interface Track {
  src: string;
  name: string;
}

export interface MusicPlayerState {
  isPlaying: boolean;
  shuffleOn: boolean;
  currentTrack: Track | null;
  currentIndex: number;
  volume: number;
  togglePlay: () => void;
  nextTrack: () => void;
  prevTrack: () => void;
  setVolume: (v: number) => void;
  toggleShuffle: () => void;
  showToast: boolean;
  toastText: string;
}

interface UseMusicPlayerOptions {
  tracks: Track[];
  storagePrefix?: string;
  defaultVolume?: number;
}

export function useMusicPlayer(opts: UseMusicPlayerOptions): MusicPlayerState {
  const { tracks, storagePrefix = 'kl-music', defaultVolume = 0.35 } = opts;
  const audioRef = useRef<HTMLAudioElement | null>(null);

  const [isPlaying, setIsPlaying] = useState(false);
  const [currentIndex, setCurrentIndex] = useState<number>(() => {
    const saved = parseInt(localStorage.getItem(`${storagePrefix}-track`) ?? '0');
    return isNaN(saved) ? 0 : saved % Math.max(tracks.length, 1);
  });
  const [shuffleOn, setShuffleOn] = useState<boolean>(() => {
    return localStorage.getItem(`${storagePrefix}-shuffle`) !== '0';
  });
  const [volume, setVolumeState] = useState<number>(() => {
    const saved = parseFloat(localStorage.getItem(`${storagePrefix}-vol`) ?? '');
    return isNaN(saved) ? defaultVolume : Math.max(0, Math.min(1, saved));
  });
  const [toastText, setToastText] = useState('');
  const [showToast, setShowToast] = useState(false);

  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const shuffleHistory = useRef<number[]>([currentIndex]);
  const shuffleHistoryPos = useRef(0);
  const intentionalPause = useRef(false);

  // Initialise audio element
  useEffect(() => {
    audioRef.current = new Audio();
    audioRef.current.volume = volume;
    return () => {
      audioRef.current?.pause();
      audioRef.current = null;
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const showToastFn = useCallback((text: string) => {
    setToastText(text);
    setShowToast(true);
    if (toastTimer.current) clearTimeout(toastTimer.current);
    toastTimer.current = setTimeout(() => setShowToast(false), 2500);
  }, []);

  const loadTrack = useCallback((index: number): number => {
    const len = tracks.length;
    if (len === 0) return 0;
    const i = ((index % len) + len) % len;
    if (audioRef.current) {
      audioRef.current.src = tracks[i]?.src ?? '';
    }
    localStorage.setItem(`${storagePrefix}-track`, String(i));
    setCurrentIndex(i);
    return i;
  }, [tracks, storagePrefix]);

  const pickNext = useCallback((): number => {
    if (!shuffleOn) return currentIndex + 1;
    if (tracks.length <= 1) return 0;
    let next: number;
    do { next = Math.floor(Math.random() * tracks.length); } while (next === currentIndex);
    return next;
  }, [shuffleOn, currentIndex, tracks.length]);

  const pickPrev = useCallback((): number => {
    if (!shuffleOn) return currentIndex - 1;
    if (shuffleHistoryPos.current > 0) {
      shuffleHistoryPos.current--;
      return shuffleHistory.current[shuffleHistoryPos.current] ?? currentIndex;
    }
    return currentIndex - 1;
  }, [shuffleOn, currentIndex]);

  const togglePlay = useCallback(() => {
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) {
      if (!audio.src || audio.src === location.href) loadTrack(currentIndex);
      audio.play().then(() => {
        showToastFn(tracks[currentIndex]?.name ?? '');
      }).catch(() => {});
    } else {
      intentionalPause.current = true;
      audio.pause();
      showToastFn('Paused');
    }
  }, [currentIndex, loadTrack, showToastFn, tracks]);

  const skipTo = useCallback((index: number) => {
    const i = loadTrack(index);
    if (shuffleOn) {
      shuffleHistoryPos.current = shuffleHistory.current.length;
      shuffleHistory.current.push(i);
    }
    audioRef.current?.play().then(() => {
      showToastFn(tracks[i]?.name ?? '');
    }).catch(() => {});
  }, [loadTrack, shuffleOn, showToastFn, tracks]);

  const nextTrack = useCallback(() => skipTo(pickNext()), [skipTo, pickNext]);
  const prevTrack = useCallback(() => skipTo(pickPrev()), [skipTo, pickPrev]);

  const setVolume = useCallback((v: number) => {
    const clamped = Math.max(0, Math.min(1, v));
    setVolumeState(clamped);
    if (audioRef.current) audioRef.current.volume = clamped;
    localStorage.setItem(`${storagePrefix}-vol`, String(clamped));
  }, [storagePrefix]);

  const toggleShuffle = useCallback(() => {
    setShuffleOn(prev => {
      const next = !prev;
      localStorage.setItem(`${storagePrefix}-shuffle`, next ? '1' : '0');
      showToastFn(next ? 'Shuffle ON' : 'Shuffle OFF');
      return next;
    });
  }, [storagePrefix, showToastFn]);

  // Wire up audio events
  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;

    const onPlay = (): void => {
      setIsPlaying(true);
      localStorage.setItem(`${storagePrefix}-playing`, '1');
    };

    const onPause = (): void => {
      setIsPlaying(false);
      if (intentionalPause.current) {
        localStorage.setItem(`${storagePrefix}-playing`, '0');
        intentionalPause.current = false;
      }
    };

    let lastTimeSave = 0;
    const onTimeUpdate = (): void => {
      const now = Date.now();
      if (now - lastTimeSave > 3000) {
        lastTimeSave = now;
        localStorage.setItem(`${storagePrefix}-time`, String(audio.currentTime));
      }
    };

    const onEnded = (): void => {
      const next = pickNext();
      const i = loadTrack(next);
      if (shuffleOn) {
        shuffleHistoryPos.current = shuffleHistory.current.length;
        shuffleHistory.current.push(i);
      }
      audio.play().then(() => {
        showToastFn(tracks[i]?.name ?? '');
      }).catch(() => {});
    };

    audio.addEventListener('play', onPlay);
    audio.addEventListener('pause', onPause);
    audio.addEventListener('timeupdate', onTimeUpdate);
    audio.addEventListener('ended', onEnded);

    return () => {
      audio.removeEventListener('play', onPlay);
      audio.removeEventListener('pause', onPause);
      audio.removeEventListener('timeupdate', onTimeUpdate);
      audio.removeEventListener('ended', onEnded);
    };
  }, [loadTrack, pickNext, showToastFn, shuffleOn, storagePrefix, tracks]);

  // Attempt resume on mount
  useEffect(() => {
    const wasPlaying = localStorage.getItem(`${storagePrefix}-playing`) === '1';
    const savedTime = parseFloat(localStorage.getItem(`${storagePrefix}-time`) ?? '0');
    const i = loadTrack(currentIndex);

    if (!wasPlaying) return;

    let resumed = false;
    const audio = audioRef.current;
    if (!audio) return;

    const doResume = (_e?: Event): void => {
      if (resumed) return;
      resumed = true;
      audio.currentTime = savedTime;
      audio.play().then(() => {
        showToastFn(tracks[i]?.name ?? '');
      }).catch(() => {});
      document.removeEventListener('click', doResume, true);
      document.removeEventListener('keydown', doResume, true);
    };

    document.addEventListener('click', doResume, true);
    document.addEventListener('keydown', doResume, true);

    audio.currentTime = savedTime;
    audio.play().then(() => {
      resumed = true;
      document.removeEventListener('click', doResume, true);
      document.removeEventListener('keydown', doResume, true);
    }).catch(() => {});

    // Remove capture listeners if the component unmounts before the user interacts
    return () => {
      document.removeEventListener('click', doResume, true);
      document.removeEventListener('keydown', doResume, true);
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return {
    isPlaying,
    shuffleOn,
    currentTrack: tracks[currentIndex] ?? null,
    currentIndex,
    volume,
    togglePlay,
    nextTrack,
    prevTrack,
    setVolume,
    toggleShuffle,
    showToast,
    toastText,
  };
}
