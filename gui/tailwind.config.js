/** @type {import('tailwindcss').Config} */
// Tailwind is scoped to the ported memory graph (and any future utility use).
// Preflight is DISABLED so Tailwind never resets/normalizes the rest of the
// rebuilt GUI -- it only emits the utility classes that are actually used,
// leaving the brand-token styling in design/tokens.css fully intact.
export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  corePlugins: {
    preflight: false
  },
  theme: {
    extend: {}
  },
  plugins: []
};
