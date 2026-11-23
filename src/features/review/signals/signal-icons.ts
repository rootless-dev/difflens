// The marks of the review's signals, as SVG markup drawn in `currentColor`, shared by the React chips and the diff
// editor's signal lane (which builds plain DOM). Fixed strings: nothing here ever comes from data.

import type { Severity } from "@/lib/api";

export interface IconShape {
  width: number;
  height: number;
  viewBox: string;
  /** The SVG's content. */
  body: string;
}

const bar = (x: number, y: number, height: number, dim: boolean) =>
  `<rect x="${x}" y="${y}" width="3" height="${height}" rx=".6"${dim ? ' opacity=".28"' : ""}/>`;

/** Three rising bars, as many filled as the severity: the count carries it without the colour. */
export function severityIcon(severity: Severity): IconShape {
  const filled = severity === "high" ? 3 : severity === "medium" ? 2 : 1;
  return {
    width: 11,
    height: 10,
    viewBox: "0 0 11 10",
    body: `<g fill="currentColor">${bar(0, 6, 4, false)}${bar(4, 3, 7, filled < 2)}${bar(8, 0, 10, filled < 3)}</g>`,
  };
}

/** A gauge with its needle at the high end: over a limit of quality. */
export const GAUGE: IconShape = {
  width: 13,
  height: 10,
  viewBox: "0 0 14 10",
  body:
    '<path d="M1.5 9a5.5 5.5 0 0 1 11 0" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>' +
    '<path d="M7 9l3.3-3.7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>' +
    '<circle cx="7" cy="9" r="1.2" fill="currentColor"/>',
};

/** An open-end wrench: a static check finding. */
export const WRENCH: IconShape = {
  width: 11,
  height: 11,
  viewBox: "0 0 12 12",
  body:
    '<path d="M11 3.8A2.8 2.8 0 1 1 8.2 1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>' +
    '<path d="M6.3 5.7L2 10" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/>',
};

/** Two overlapping sheets: duplicated code. */
export const COPY: IconShape = {
  width: 11,
  height: 11,
  viewBox: "0 0 12 12",
  body:
    '<rect x="1" y="3.6" width="6.4" height="7.4" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3"/>' +
    '<path d="M4.4 3.4V2.2c0-.66.54-1.2 1.2-1.2h4.2c.66 0 1.2.54 1.2 1.2v5c0 .66-.54 1.2-1.2 1.2H8.6" fill="none" ' +
    'stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
};

/** A bug: a mutant that survived, a change planted in the code that no test caught. */
export const BUG: IconShape = {
  width: 11,
  height: 11,
  viewBox: "0 0 12 12",
  body:
    '<ellipse cx="6" cy="7.3" rx="2.7" ry="3.5" fill="none" stroke="currentColor" stroke-width="1.3"/>' +
    '<path d="M4.7 3.1a1.3 1.3 0 0 1 2.6 0" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>' +
    '<path d="M1 5.6h2.2M8.8 5.6H11M1 9h2.2M8.8 9H11" fill="none" stroke="currentColor" stroke-width="1.2" ' +
    'stroke-linecap="round"/>',
};

/** A lab flask struck through: no test executes the function. */
export const NO_TEST: IconShape = {
  width: 11,
  height: 11,
  viewBox: "0 0 12 12",
  body:
    '<path d="M4.4 1.3h3.2 M5.2 1.3v3.4L2.3 10.4h7.4L6.8 4.7V1.3" fill="none" stroke="currentColor" ' +
    'stroke-width="1.15" stroke-linejoin="round" stroke-linecap="round"/>' +
    '<path d="M1.4 1.6l9.2 9" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
};

/** The icon as an SVG element's markup. */
export function iconMarkup(icon: IconShape): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" aria-hidden="true" width="${icon.width}" height="${icon.height}" ` +
    `viewBox="${icon.viewBox}">${icon.body}</svg>`
  );
}
