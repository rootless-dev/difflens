// What React Flow needs from the browser that jsdom lacks, so tests can render the real canvas.
// Import it from a test file before rendering a graph. Nodes get the size of their inline style
// (React Flow writes the width and height it is given), and the pane gets a fixed 1000 x 600.

class ResizeObserverStub {
  constructor(private readonly callback: ResizeObserverCallback) {}

  observe(target: Element) {
    this.callback([{ target } as ResizeObserverEntry], this as unknown as ResizeObserver);
  }

  unobserve() {}

  disconnect() {}
}

class DOMMatrixReadOnlyStub {
  m22: number;

  constructor(transform?: string) {
    const scale = transform?.match(/scale\(([\d.]+)\)/)?.[1];
    this.m22 = scale !== undefined ? Number(scale) : 1;
  }
}

const sizeOf = (element: HTMLElement, side: "width" | "height") =>
  Number.parseFloat(element.style[side]) || (element.classList.contains("react-flow") ? (side === "width" ? 1000 : 600) : 1);

globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
globalThis.DOMMatrixReadOnly ??= DOMMatrixReadOnlyStub as unknown as typeof DOMMatrixReadOnly;

Object.defineProperties(HTMLElement.prototype, {
  offsetWidth: {
    configurable: true,
    get(this: HTMLElement) {
      return sizeOf(this, "width");
    },
  },
  offsetHeight: {
    configurable: true,
    get(this: HTMLElement) {
      return sizeOf(this, "height");
    },
  },
});

(SVGElement.prototype as SVGGraphicsElement).getBBox ??= () => ({ x: 0, y: 0, width: 0, height: 0 }) as DOMRect;

export {};
