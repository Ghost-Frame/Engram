import { render } from '@testing-library/react';
import { expect, it } from 'vitest';
import { MemoryBackground } from './MemoryBackground';

// The background mounts a decorative, aria-hidden canvas.
it('renders an aria-hidden canvas', () => {
  const { container } = render(<MemoryBackground />);
  const canvas = container.querySelector('canvas');
  expect(canvas).not.toBeNull();
  expect(canvas?.getAttribute('aria-hidden')).toBe('true');
});
