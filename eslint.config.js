// Lint configuration for the frontend (src/) and the JS/TS analyzer
// (analyzers/jsts/). Kept to the recommended rule sets.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import { defineConfig, globalIgnores } from "eslint/config";
import globals from "globals";
import tseslint from "typescript-eslint";

// A leading underscore marks a binding that is unused on purpose.
const unusedVars = [
  "error",
  {
    argsIgnorePattern: "^_",
    varsIgnorePattern: "^_",
    caughtErrorsIgnorePattern: "^_",
    destructuredArrayIgnorePattern: "^_",
  },
];

export default defineConfig([
  globalIgnores([
    "dist",
    "coverage",
    "src-tauri",
    "design",
    "analyzers/*",
    "!analyzers/jsts",
    // Fixture projects hold findings on purpose: the analyzer's `check` reports them.
    "analyzers/jsts/testdata",
  ]),
  {
    files: ["src/**/*.{ts,tsx}", "vite.config.ts"],
    extends: [js.configs.recommended, tseslint.configs.recommended, reactHooks.configs.flat.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    rules: {
      "@typescript-eslint/no-unused-vars": unusedVars,
    },
  },
  {
    files: ["analyzers/jsts/**/*.js"],
    extends: [js.configs.recommended],
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: globals.node,
    },
    rules: {
      "no-unused-vars": unusedVars,
    },
  },
]);
