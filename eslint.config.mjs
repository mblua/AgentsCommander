// ESLint first slice (#4460): Solid-specific rules only.
// Each rule has no SonarCloud equivalent; see issue #4460 plan.
import tseslint from "typescript-eslint";
import solid from "eslint-plugin-solid";

export default [
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    linterOptions: { reportUnusedDisableDirectives: "off" },
    plugins: { solid },
    rules: {
      "solid/no-destructure": "error",
      "solid/jsx-no-script-url": "error",
      "solid/event-handlers": "error",
      "solid/imports": "error",
      "solid/no-react-deps": "error",
      "solid/no-react-specific-props": "error",
    },
  },
];
