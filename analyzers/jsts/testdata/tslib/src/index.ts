export { Circle, Square, totalArea, largest } from "./shapes";
export type { Shape } from "./shapes";
export * from "./repository";
export { round as roundTo } from "./util";

export default function version(): string {
  return "1.0.0";
}
