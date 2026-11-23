import { describe, expect, it } from "vitest";

import { adjacentPath, fileLabel, filterFiles, groupFiles, orderedPaths } from "@/features/review/file-tree";
import { makeChangedFile } from "@/test/fixtures";

const shape = (files: ReturnType<typeof makeChangedFile>[]) =>
  groupFiles(files).map((group) => [group.dir, group.files.map((file) => file.path)]);

describe("groupFiles", () => {
  it("groups by directory, root first, nested directories right after their parent, files by name", () => {
    const files = [
      "internal/refund/service.go",
      "go.mod",
      "internal/refund-old/x.go",
      "internal/refund/handler.go",
      "internal/refund/sub/deep.go",
      "README.md",
      "cmd/api/main.go",
      "internal/payment/repo.go",
    ].map((path) => makeChangedFile(path));

    expect(shape(files)).toEqual([
      ["", ["README.md", "go.mod"]],
      ["cmd/api", ["cmd/api/main.go"]],
      ["internal/payment", ["internal/payment/repo.go"]],
      ["internal/refund", ["internal/refund/handler.go", "internal/refund/service.go"]],
      ["internal/refund/sub", ["internal/refund/sub/deep.go"]],
      ["internal/refund-old", ["internal/refund-old/x.go"]],
    ]);
  });

  it("puts a renamed file in the directory of its new path", () => {
    const files = [makeChangedFile("pkg/new/name.go", { status: "renamed", oldPath: "pkg/old/name.go" })];

    expect(shape(files)).toEqual([["pkg/new", ["pkg/new/name.go"]]]);
  });

  it("returns nothing for no files", () => {
    expect(groupFiles([])).toEqual([]);
  });
});

describe("filterFiles", () => {
  const files = [
    makeChangedFile("internal/refund/Service.go"),
    makeChangedFile("internal/payment/repo.go"),
    makeChangedFile("pkg/ledger.go", { status: "renamed", oldPath: "pkg/refund_ledger.go" }),
  ];

  it("matches the path or the old path, ignoring case and surrounding spaces", () => {
    expect(filterFiles(files, "  REFUND ").map((file) => file.path)).toEqual([
      "internal/refund/Service.go",
      "pkg/ledger.go",
    ]);
    expect(filterFiles(files, "service").map((file) => file.path)).toEqual(["internal/refund/Service.go"]);
  });

  it("keeps everything for an empty query", () => {
    expect(filterFiles(files, " ")).toBe(files);
  });
});

describe("adjacentPath", () => {
  const paths = ["a", "b", "c"];

  it("moves forward and back and stays at either end", () => {
    expect(adjacentPath(paths, "a", 1)).toBe("b");
    expect(adjacentPath(paths, "b", -1)).toBe("a");
    expect(adjacentPath(paths, "c", 1)).toBe("c");
    expect(adjacentPath(paths, "a", -1)).toBe("a");
  });

  it("starts from either end when the current path is not in the list", () => {
    expect(adjacentPath(paths, "hidden", 1)).toBe("a");
    expect(adjacentPath(paths, null, -1)).toBe("c");
    expect(adjacentPath([], "a", 1)).toBeNull();
  });

  it("follows the grouped order", () => {
    const groups = groupFiles([makeChangedFile("z/1.go"), makeChangedFile("root.go"), makeChangedFile("a/2.go")]);
    expect(orderedPaths(groups)).toEqual(["root.go", "a/2.go", "z/1.go"]);
  });
});

describe("fileLabel", () => {
  it("shows the file name", () => {
    expect(fileLabel(makeChangedFile("internal/refund/service.go"))).toEqual({ name: "service.go", renamedFrom: null });
  });

  it("shows the old file name for a rename in the same directory, and the old path for a move", () => {
    expect(fileLabel(makeChangedFile("pkg/b.go", { status: "renamed", oldPath: "pkg/a.go" }))).toEqual({
      name: "b.go",
      renamedFrom: "a.go",
    });
    expect(fileLabel(makeChangedFile("pkg/b.go", { status: "renamed", oldPath: "old/b.go" }))).toEqual({
      name: "b.go",
      renamedFrom: "old/b.go",
    });
  });
});
