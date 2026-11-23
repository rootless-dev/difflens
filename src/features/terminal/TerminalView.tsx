import "@xterm/xterm/css/xterm.css";

import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef } from "react";

import { useTerminals } from "@/features/terminal/terminals";
import { isTerminalShortcut } from "@/features/terminal/terminal-text";
import { TERMINAL_FONT, terminalTheme } from "@/features/terminal/terminal-theme";
import type { Theme } from "@/hooks/use-theme";
import { i18n } from "@/i18n";
import type { TerminalSession } from "@/lib/api";

interface TerminalViewProps {
  session: TerminalSession;
  /** The app's theme: the terminal's colours follow it. */
  theme: Theme;
  /** Takes the keyboard when it mounts, after a click that opened or chose this session. */
  autoFocus: boolean;
}

/**
 * One session drawn by xterm.js. The session lives in the core: mounting attaches to it, which replays what it
 * printed (its scrollback), and unmounting only stops drawing, so it can come back on another screen. The terminal
 * fits its box and tells the core its size; ⌃` and ⌃⇧` are left to the footer even while the terminal has the keyboard.
 * Copy and paste are the system's (⌘C with a selection, ⌘V); links open in the browser.
 */
export function TerminalView({ session, theme, autoFocus }: TerminalViewProps) {
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const { view, write, resize } = useTerminals();
  const focusOnMount = useRef(autoFocus);
  const id = session.id;

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    const term = new Terminal({
      fontFamily: TERMINAL_FONT,
      fontSize: 12,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 10_000,
      theme: terminalTheme(),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(
      new WebLinksAddon((_event, uri) => {
        void openUrl(uri).catch(() => {});
      }),
    );
    term.open(element);
    term.attachCustomKeyEventHandler((event) => !isTerminalShortcut(event));
    terminal.current = term;

    let size = "";
    const fitToBox = () => {
      try {
        fit.fit();
      } catch {
        // Not laid out yet (hidden): the next resize fits it.
        return;
      }
      const next = `${term.cols}x${term.rows}`;
      if (next !== size) {
        size = next;
        resize(id, term.cols, term.rows);
      }
    };
    fitToBox();
    // Measured with the app's font once it has loaded.
    void document.fonts?.ready.then(fitToBox);
    const observer = new ResizeObserver(fitToBox);
    observer.observe(element);

    const input = term.onData((data) => write(id, data));
    const stop = view(id, {
      output: (data) => term.write(data),
      event: (event) => {
        if (event.type !== "exited") return;
        const text =
          event.code === null || event.code === 0
            ? i18n.t("terminal.exited")
            : i18n.t("terminal.exitedWithCode", { code: event.code });
        term.write(`\r\n\x1b[2m${text}\x1b[0m\r\n`);
      },
    });
    if (focusOnMount.current) term.focus();

    return () => {
      stop();
      observer.disconnect();
      input.dispose();
      term.dispose();
      terminal.current = null;
    };
  }, [id, view, write, resize]);

  useEffect(() => {
    if (terminal.current) terminal.current.options.theme = terminalTheme();
  }, [theme]);

  return <div ref={host} data-testid="terminal-view" className="h-full min-h-0 w-full bg-background py-2 pl-3.5" />;
}
