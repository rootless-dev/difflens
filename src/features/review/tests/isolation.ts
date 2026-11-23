import { i18n } from "@/i18n";
import type { Isolation } from "@/lib/api";

export interface IsolationLook {
  value: Isolation;
  /** As the isolation choice names it. */
  label: string;
  /** What it means for the code that runs, worded like Project settings. */
  description: string;
  /** Item of the Run again menu. */
  again: string;
  /** Said of a finished run. */
  ranIn: string;
  /** Said of a run going on: "Running the tests in the sandbox". */
  running: string;
}

/** The sandbox first: it is the default and the safer of the two. */
export const ISOLATIONS: readonly Isolation[] = ["sandbox", "process"];

/** How an isolation is worded, in the language shown. */
export function isolationLook(isolation: Isolation): IsolationLook {
  const value = ISOLATIONS.includes(isolation) ? isolation : ISOLATIONS[0];
  return {
    value,
    label: i18n.t(`tests.isolation.${value}.label`),
    description: i18n.t(`tests.isolation.${value}.description`),
    again: i18n.t(`tests.isolation.${value}.again`),
    ranIn: i18n.t(`tests.isolation.${value}.ranIn`),
    running: i18n.t(`tests.isolation.${value}.running`),
  };
}
