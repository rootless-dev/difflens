import { type ReactNode, useId } from "react";

/** One section of Preferences, named by its heading; each setting in it is a row (the prototype's `.stepr`). */
export function PreferencesSection({ title, children }: { title: string; children: ReactNode }) {
  const id = useId();
  return (
    <section aria-labelledby={id}>
      <h2 id={id} className="mt-[30px] mb-2.5 text-sm font-medium text-muted-foreground">
        {title}
      </h2>
      <div className="border-t">{children}</div>
    </section>
  );
}
