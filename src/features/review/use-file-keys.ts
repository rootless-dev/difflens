import { useEffect, useRef } from "react";

import { isTyping } from "@/lib/utils";

const STEPS: Record<string, 1 | -1> = { j: 1, "]": 1, k: -1, "[": -1 };

/**
 * Calls `onStep` with 1 for j or ], and -1 for k or [, unless the user is typing in a field. Listens only while
 * `enabled`, so a view kept mounted behind another tab does not react.
 */
export function useFileKeys(onStep: (step: 1 | -1) => void, enabled = true) {
  const latest = useRef(onStep);
  useEffect(() => {
    latest.current = onStep;
  });

  useEffect(() => {
    if (!enabled) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || isTyping(event.target)) {
        return;
      }
      const step = STEPS[event.key];
      if (step) {
        event.preventDefault();
        latest.current(step);
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [enabled]);
}
