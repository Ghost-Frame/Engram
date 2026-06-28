import { useQuery } from '@tanstack/react-query';
import { getMusicManifest, musicSrc } from '$lib/api/memory';
import { MusicPlayer } from './MusicPlayer';

// Mount the ambient music player only when the server exposes tracks. The
// manifest is empty unless KLEOS_GUI_MUSIC_DIR is configured, so the player
// stays hidden for users who have not set up music.
export function KleosMusic() {
  // Fetch the track manifest once; it changes only when the server restarts.
  const manifest = useQuery({
    queryFn: getMusicManifest,
    queryKey: ['music', 'manifest'],
    staleTime: Infinity,
  });

  // Map server manifest entries to the MusicPlayer Track shape.
  const tracks = (manifest.data ?? []).map((t) => ({ name: t.name, src: musicSrc(t.src) }));

  // Render nothing until tracks are available.
  if (tracks.length === 0) {
    return null;
  }

  return <MusicPlayer tracks={tracks} storagePrefix="kl-music" />;
}
