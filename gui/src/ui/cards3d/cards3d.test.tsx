import { render, screen, fireEvent } from '@testing-library/react';
import { FloatingCard } from './FloatingCard';

// A non-empty card fires onClick and shows its count.
it('renders a clickable card with count', () => {
  const onClick = vi.fn();
  render(<FloatingCard title="March" count={240} index={2} onClick={onClick} />);
  expect(screen.getByText('March')).toBeInTheDocument();
  expect(screen.getByText('240')).toBeInTheDocument();
  fireEvent.click(screen.getByText('March'));
  expect(onClick).toHaveBeenCalledOnce();
});

// An empty card does not fire onClick.
it('does not fire onClick when empty', () => {
  const onClick = vi.fn();
  render(<FloatingCard title="Feb" count={0} isEmpty index={1} onClick={onClick} />);
  fireEvent.click(screen.getByText('Feb'));
  expect(onClick).not.toHaveBeenCalled();
});
