# NeuralCanvas design direction

Transcribed from the owner's Phase 5 brief and `docs/UX-PRINCIPLES.md`. The rules in UX-PRINCIPLES are
the acceptance criteria; this file only fixes the choices they leave open.

## Identity

- **Product character:** a quiet, fast drawing tool for people who think in boxes and arrows. The
  canvas is the product; chrome is furniture that stays out of the way.
- **Typography:** the NODAYSIDLE pair, as on nodaysidle.com: **Manrope** for interface text, **DM Mono**
  for numbers, shortcuts and measurements. Both SIL OFL 1.1, bundled with their licences.
- **Theme:** dark first (a drawing surface at night is the common case), light from the desktop's
  `color-scheme` portal setting, or forced in Preferences.

## Colour

- Neutrals carry the chrome: panel, raised control, hairline border, text, muted text.
- **One accent**, the selection blue already on the canvas (`#5e9dff`-ish in sRGB): focus rings, the
  active tool, the selection, primary actions. Nowhere else.
- Two state colours with a job: amber for "uncertain recognition" and red for errors and the unsaved
  banner. They mark state, never decoration.
- Ink is theme-aware: in light theme the canvas shows ink with its lightness inverted, so the same
  sketch reads on both backgrounds.

## Shape and depth

- 4 px spacing grid. Panels 8 px radius, controls 6 px, nothing pill-shaped.
- **No shadows** (owner: no fuzzy drop shadows). Floating panels separate from the canvas with a 1 px
  border and a solid fill. No blur, no glow, no gradients.

## Motion

- 120–200 ms ease-out, only for things that appear or move (palette fade while drawing, toasts, the
  sidebar). Nothing loops. Reduced motion turns all of it into instant changes.

## Controls

- Text controls, not icon fonts: tools are named ("Pen", "Eraser") with their key beside them in DM Mono.
- Every control has hover, pressed, focused and disabled states, a label, and a keyboard path.
