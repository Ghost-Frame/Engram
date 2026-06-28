// Adapted from an internal effects library.
// ========== MUSIC PLAYER REACT COMPONENT ==========
// Fixed-position player wrapping the vanilla hook logic

import React from 'react';
import { useMusicPlayer } from './useMusicPlayer';
import './music.css';

export interface Track {
  src: string;
  name: string;
}

export interface MusicPlayerProps {
  tracks: Track[];
  /** localStorage key prefix. Default: 'sa-music' */
  storagePrefix?: string;
  /** Initial volume 0-1. Default: 0.35 */
  defaultVolume?: number;
  /** Extra className on the wrapper */
  className?: string;
}

const PlayIcon = (): React.ReactElement => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <polygon points="5 3 19 12 5 21 5 3"/>
  </svg>
);

const EqBars = (): React.ReactElement => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
    <line className="eq-bar" x1="4" y1="20" x2="4" y2="10"/>
    <line className="eq-bar" x1="9" y1="20" x2="9" y2="6"/>
    <line className="eq-bar" x1="14" y1="20" x2="14" y2="14"/>
    <line className="eq-bar" x1="19" y1="20" x2="19" y2="8"/>
  </svg>
);

const SkipBackIcon = (): React.ReactElement => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <polygon points="19 20 9 12 19 4 19 20"/>
    <line x1="5" y1="19" x2="5" y2="5"/>
  </svg>
);

const SkipForwardIcon = (): React.ReactElement => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <polygon points="5 4 15 12 5 20 5 4"/>
    <line x1="19" y1="5" x2="19" y2="19"/>
  </svg>
);

const ShuffleIcon = (): React.ReactElement => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <polyline points="16 3 21 3 21 8"/>
    <line x1="4" y1="20" x2="21" y2="3"/>
    <polyline points="21 16 21 21 16 21"/>
    <line x1="15" y1="15" x2="21" y2="21"/>
    <line x1="4" y1="4" x2="9" y2="9"/>
  </svg>
);

function VolumeIcon({ volume }: { volume: number }): React.ReactElement {
  if (volume === 0) {
    return (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <polygon points="11 5 6 9 2 9 2 15 6 15 11 19"/>
        <line x1="23" y1="9" x2="17" y2="15"/>
        <line x1="17" y1="9" x2="23" y2="15"/>
      </svg>
    );
  }
  if (volume <= 0.5) {
    return (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <polygon points="11 5 6 9 2 9 2 15 6 15 11 19"/>
        <path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polygon points="11 5 6 9 2 9 2 15 6 15 11 19"/>
      <path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>
      <path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>
    </svg>
  );
}

export function MusicPlayer(props: MusicPlayerProps): React.ReactElement {
  const { tracks, storagePrefix, defaultVolume, className } = props;

  // Pass props directly; the hook supplies its own defaults for optional fields
  const {
    isPlaying, shuffleOn, volume,
    togglePlay, nextTrack, prevTrack,
    setVolume, toggleShuffle,
    showToast, toastText,
  } = useMusicPlayer({ tracks, storagePrefix, defaultVolume });

  const [volPinned, setVolPinned] = React.useState(false);

  return (
    <>
      <div className={['music-player-fixed', className].filter(Boolean).join(' ')}>
        <div className="music-controls">
          <button
            className="music-skip"
            onClick={prevTrack}
            aria-label="Previous track"
          >
            <SkipBackIcon />
          </button>
          <button
            className={['music-toggle', isPlaying ? 'is-playing' : ''].join(' ').trim()}
            onClick={togglePlay}
            aria-label={isPlaying ? 'Pause' : 'Play'}
          >
            <span className="icon-playing"><EqBars /></span>
            <span className="icon-paused"><PlayIcon /></span>
          </button>
          <button
            className="music-skip"
            onClick={nextTrack}
            aria-label="Next track"
          >
            <SkipForwardIcon />
          </button>
          <button
            className={['music-shuffle', shuffleOn ? 'active' : ''].join(' ').trim()}
            onClick={toggleShuffle}
            aria-label="Toggle shuffle"
          >
            <ShuffleIcon />
          </button>
          <div
            className={['music-vol', volPinned ? 'pinned' : ''].join(' ').trim()}
            onMouseDown={() => setVolPinned(true)}
            onMouseUp={() => setVolPinned(false)}
          >
            <VolumeIcon volume={volume} />
            <input
              type="range"
              min="0"
              max="1"
              step="0.01"
              value={volume}
              onChange={e => setVolume(parseFloat(e.target.value))}
              aria-label="Volume"
            />
          </div>
        </div>
      </div>
      <div className={['music-toast', showToast ? 'show' : ''].join(' ').trim()} aria-live="polite">
        {toastText}
      </div>
    </>
  );
}
