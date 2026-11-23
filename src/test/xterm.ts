// A stand-in for xterm.js in jsdom, which has no canvas or layout: it keeps what it was told to write and lets a test
// type into it. Tests replace the module with `vi.mock("@xterm/xterm", () => import("@/test/xterm"))`.

const decoder = new TextDecoder();

export class Terminal {
  static instances: Terminal[] = [];

  cols = 80;
  rows = 24;
  options: Record<string, unknown>;
  /** Everything written, decoded. */
  text = "";
  disposed = false;
  focused = false;
  element: HTMLElement | null = null;
  private dataListeners: ((data: string) => void)[] = [];
  private keyHandler: ((event: KeyboardEvent) => boolean) | null = null;

  constructor(options: Record<string, unknown> = {}) {
    this.options = { ...options };
    Terminal.instances.push(this);
  }

  open(element: HTMLElement) {
    this.element = element;
  }

  loadAddon() {}

  write(data: string | Uint8Array) {
    this.text += typeof data === "string" ? data : decoder.decode(data);
  }

  onData(listener: (data: string) => void) {
    this.dataListeners.push(listener);
    return {
      dispose: () => {
        this.dataListeners = this.dataListeners.filter((other) => other !== listener);
      },
    };
  }

  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) {
    this.keyHandler = handler;
  }

  focus() {
    this.focused = true;
  }

  dispose() {
    this.disposed = true;
  }

  /** What typing `data` into the terminal sends. */
  type(data: string) {
    for (const listener of this.dataListeners) listener(data);
  }

  /** Whether the terminal would handle the key itself. */
  handles(event: KeyboardEvent) {
    return this.keyHandler ? this.keyHandler(event) : true;
  }

  /** The terminals not disposed yet. */
  static live() {
    return Terminal.instances.filter((terminal) => !terminal.disposed);
  }
}
