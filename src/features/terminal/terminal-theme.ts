import type { ITheme } from "@xterm/xterm";

/** The app's monospace font, as the diff and the code blocks use it. */
export const TERMINAL_FONT = '"IBM Plex Mono", ui-monospace, monospace';

/**
 * The terminal's colours, read from the app's CSS variables (index.css) so they follow the theme: the background and
 * text of the app, its selection colour, and the sixteen ANSI colours mapped onto its palette (red the deletions,
 * green the additions, yellow the modifications, blue the accent, magenta what a model wrote, cyan the strings of the
 * syntax colours). Black and white swap ends with the theme so both stay readable on its background.
 */
export function terminalTheme(): ITheme {
  const root = document.documentElement;
  const style = getComputedStyle(root);
  const token = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
  const dark = root.classList.contains("dark");
  const background = token("--background", dark ? "#151925" : "#eceff5");
  const foreground = token("--foreground", dark ? "#e7e9f2" : "#1b2132");
  const muted = token("--muted-foreground", dark ? "#8d96ae" : "#5d6780");
  const faint = token("--faint", dark ? "#5c6580" : "#98a1b7");
  const line = token("--border", dark ? "#2c344c" : "#d8ddea");
  const red = token("--del", "#f27d72");
  const green = token("--add", "#56cf97");
  const yellow = token("--mod", "#e9b85c");
  const blue = token("--primary", "#7ea6ff");
  const magenta = token("--ai", "#d992e0");
  const cyan = token("--syntax-string", "#7cc7d8");
  return {
    background,
    foreground,
    cursor: foreground,
    cursorAccent: background,
    selectionBackground: token("--primary-soft", "rgba(126,166,255,.14)"),
    selectionInactiveBackground: token("--primary-soft", "rgba(126,166,255,.14)"),
    black: dark ? line : foreground,
    brightBlack: dark ? faint : muted,
    white: dark ? muted : line,
    brightWhite: dark ? foreground : faint,
    red,
    brightRed: red,
    green,
    brightGreen: green,
    yellow,
    brightYellow: yellow,
    blue,
    brightBlue: blue,
    magenta,
    brightMagenta: magenta,
    cyan,
    brightCyan: cyan,
  };
}
