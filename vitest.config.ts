import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vitest/config";

// The frontend's pure logic: date buckets, model resolution, list selection,
// the cloud hours maths. `npm run check` only type-checks; this is the half that
// actually runs the code. Same idiom as server/: `vitest run`, tests colocated
// as `foo.test.ts`, explicit imports (no globals).
export default defineConfig({
  // The SvelteKit plugin is not optional here, for two reasons:
  //
  //  1. `$lib` is a SvelteKit-provided alias; nothing declares it by hand.
  //  2. `listSelection.svelte.ts` is a runes module: `$state` is a compiler
  //     construct, not a function. Without the Svelte plugin's compileModule()
  //     transform the file throws `$state is not defined` the moment it is
  //     imported. A hand-rolled `resolve.alias` would satisfy (1) and still fail
  //     on (2).
  plugins: [sveltekit()],

  resolve: {
    // The compiled runes module imports `svelte/internal/client`. Node
    // resolution would otherwise hand it `svelte/internal/server`, where the
    // state primitives are inert no-ops and every assertion quietly passes
    // against a dead object. No jsdom needed; none of this touches the DOM.
    conditions: ["browser"],
  },

  test: {
    include: ["src/**/*.test.ts"],
  },
});
