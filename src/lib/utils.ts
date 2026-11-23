export { cn } from "cn"

/** True when a key pressed in `target` is text being typed: an input, a text area, a select or editable content. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)
}
