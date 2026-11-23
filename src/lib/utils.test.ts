import { describe, expect, it } from "vitest";

import { cn } from "@/lib/utils";

describe("cn", () => {
  it("joins class names and resolves conflicting Tailwind utilities", () => {
    expect(cn("px-2", "font-medium", "px-4")).toBe("font-medium px-4");
  });
});
