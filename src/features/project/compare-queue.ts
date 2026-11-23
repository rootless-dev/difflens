import { i18n } from "@/i18n";
import { type CompareSummary, isApiError } from "@/lib/api";
import { errorMessage } from "@/lib/format";

export interface CompareTarget {
  branch: string;
  base: string;
  /** Commit the base points at: a base that moved is compared again. */
  baseSha: string;
  /** Head commit of the branch: a branch that moved is compared again. */
  headSha: string;
}

export type CompareState =
  | { status: "loading" }
  | { status: "ready"; data: CompareSummary }
  /** `short` fits in a row; `message` is the full text, for a title. */
  | { status: "error"; short: string; message: string };

export function compareKey({ branch, base, baseSha, headSha }: CompareTarget): string {
  // A NUL byte can appear neither in a git ref name nor in a sha.
  return `${branch}\u0000${base}\u0000${baseSha}\u0000${headSha}`;
}

const SHORT_ERRORS = {
  no_common_history: "project.compare.noCommonHistory",
  unknown_branch: "project.compare.unknownBranch",
} as const;

/** In the language shown when the comparison failed. */
export function shortCompareError(error: unknown): string {
  const known = isApiError(error) && error.code in SHORT_ERRORS;
  return i18n.t(known ? SHORT_ERRORS[error.code as keyof typeof SHORT_ERRORS] : "project.compare.failed");
}

/**
 * Runs comparisons a few at a time and remembers every result, keyed by branch, base, base commit
 * and head commit, so a row that comes back into view, or a base switched back, is not compared
 * again. Errors are remembered too: they come back only when the branch or its base moves.
 */
export class CompareQueue {
  private readonly states = new Map<string, CompareState>();
  private queued: CompareTarget[] = [];
  private running = 0;
  private version = 0;
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly compare: (target: CompareTarget) => Promise<CompareSummary>,
    private readonly concurrency = 4,
  ) {}

  get(target: CompareTarget): CompareState | undefined {
    return this.states.get(compareKey(target));
  }

  /**
   * Declares the comparisons wanted now, in display order. Unknown ones are queued; queued ones
   * that are no longer wanted are dropped before they start. Results and running requests are kept.
   */
  want(targets: CompareTarget[]): void {
    const wanted = new Set(targets.map(compareKey));
    let changed = false;

    const stillWanted: CompareTarget[] = [];
    for (const target of this.queued) {
      if (wanted.has(compareKey(target))) {
        stillWanted.push(target);
      } else {
        this.states.delete(compareKey(target));
        changed = true;
      }
    }
    this.queued = stillWanted;

    for (const target of targets) {
      const key = compareKey(target);
      if (!this.states.has(key)) {
        this.states.set(key, { status: "loading" });
        this.queued.push(target);
        changed = true;
      }
    }

    this.pump();
    if (changed) this.emit();
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Changes every time a state changes; a snapshot for useSyncExternalStore. */
  getVersion = (): number => this.version;

  private pump(): void {
    while (this.running < this.concurrency && this.queued.length > 0) {
      const target = this.queued.shift() as CompareTarget;
      this.running += 1;
      this.compare(target)
        .then(
          (data): CompareState => ({ status: "ready", data }),
          (error: unknown): CompareState => ({
            status: "error",
            short: shortCompareError(error),
            message: errorMessage(error),
          }),
        )
        .then((state) => {
          this.states.set(compareKey(target), state);
          this.running -= 1;
          this.pump();
          this.emit();
        });
    }
  }

  private emit(): void {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }
}
