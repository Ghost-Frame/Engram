import { useEffect, useRef } from 'react';
import { BackgroundRenderer } from './background';

// Render the ambient starfield backdrop behind the Memory hub. The canvas is
// decorative, sits below content, and pauses when the tab is hidden or the
// user prefers reduced motion.
//
// NOTE: BackgroundRenderer (from ./background) auto-starts its own rAF loop
// in the constructor and exposes only destroy() for teardown -- there is no
// start()/stop() pair. Visibility toggling is handled by destroying and
// recreating the renderer.
export function MemoryBackground() {
  // Ref to the underlying canvas DOM element.
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // Ref to the live renderer so we can destroy it across closures.
  const rendererRef = useRef<BackgroundRenderer | null>(null);

  useEffect(() => {
    // Skip animation entirely when the user prefers reduced motion.
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (reduce) {
      return;
    }

    // Start a renderer on the given canvas, storing the handle for cleanup.
    // Wrapped in try-catch because BackgroundRenderer throws when the canvas
    // cannot provide a 2D context (e.g. jsdom, privacy browsers).
    const start = (): void => {
      if (!canvasRef.current || rendererRef.current) return;
      try {
        rendererRef.current = new BackgroundRenderer({
          canvas: canvasRef.current,
          type: 'starfield',
        });
      } catch {
        // Context unavailable -- leave the canvas element in place but skip animation.
      }
    };

    // Destroy the live renderer and clear the ref.
    const stop = (): void => {
      rendererRef.current?.destroy();
      rendererRef.current = null;
    };

    start();

    // Pause rendering while the tab is not visible to save CPU.
    const onVisibility = (): void => {
      if (document.hidden) {
        stop();
      } else {
        start();
      }
    };

    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      stop();
    };
  }, []);

  return (
    <canvas
      ref={canvasRef}
      aria-hidden="true"
      style={{ position: 'fixed', inset: 0, zIndex: 0, pointerEvents: 'none', opacity: 0.5 }}
    />
  );
}
