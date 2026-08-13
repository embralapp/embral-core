// The anti-rot guard (docs/copy.md). Two checks over the Svelte source, plus
// a smoke test:
//
//  1. No user-facing string literal in markup; copy belongs in the catalog.
//     Parses each component and walks the FRAGMENT only (never the <script>),
//     so config sentinels, invoke() names, event names, discriminants, and
//     console text are out of scope by construction. Flags copy-bearing
//     attributes (by an allowlist of attribute names) and text nodes, never
//     descending into attribute values (class="flex ..." is not copy).
//
//  2. No non-reactive catalog alias: a top-level `const t = copy.x` reads the
//     getter once and a future locale swap never reaches it. Only
//     `$derived(copy.x)` is sanctioned. Nested reads (inside $derived.by or a
//     function body) are fine and not flagged.
//
//  3. A smoke test that the Svelte AST still has the shape checks 1–2 rely on,
//     so a compiler upgrade can't make the guard silently match nothing.
//
// Runs under `npm test` only; tsconfig excludes tests from svelte-check.

import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';
import { parse } from 'svelte/compiler';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..', '..');
const roots = ['src/routes', 'src/lib'];

// Attribute names that carry copy. Whitelist, never blacklist: value=, class=,
// style=, href=, id=, data-* are never considered.
const COPY_ATTRS = new Set([
  'title',
  'label',
  'placeholder',
  'description',
  'aria-label',
  'ariaLabel',
  'alt',
  'sub',
  'subtitle',
  'heading',
  'confirmLabel',
  'cancelText',
  'confirmText',
  'emptyText',
  'text',
  'providerLabel',
  'disabledNote',
  'cloudNote',
  'fallbackLabel',
  'body'
]);

const HAS_LETTERS = /[A-Za-z]{2,}/;

// The only sanctioned bare strings in markup: the brand wordmark, which must
// never be translated. Keyed by repo-relative POSIX path → exact trimmed
// strings. It should stay tiny; every addition is a review conversation.
const ALLOW: Record<string, string[]> = {
  'src/lib/components/onboarding/Onboarding.svelte': ['embral'],
  'src/lib/components/settings/AboutSection.svelte': ['embral'],
  'src/lib/components/shell/TitleBar.svelte': ['embral']
};

function svelteFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    // The open-core tree drops src/lib/cloud/ entirely; a missing dir is
    // never walked (cloud-seam.md). Present here, it is checked too.
    if (entry.isDirectory()) svelteFiles(full, acc);
    else if (entry.name.endsWith('.svelte')) acc.push(full);
  }
  return acc;
}

/** Every literal-string flag in a component's fragment. */
function fragmentStringLiterals(source: string): string[] {
  const ast = parse(source, { modern: true });
  const found: string[] = [];
  (function walk(node: unknown): void {
    if (!node || typeof node !== 'object') return;
    const n = node as Record<string, unknown>;
    if (n.type === 'Attribute') {
      const value = n.value;
      if (
        COPY_ATTRS.has(n.name as string) &&
        Array.isArray(value) &&
        value.length === 1 &&
        (value[0] as Record<string, unknown>)?.type === 'Text'
      ) {
        const data = (value[0] as { data: string }).data;
        if (HAS_LETTERS.test(data)) found.push(data.trim());
      }
      // Never descend into an attribute's value.
      return;
    }
    if (n.type === 'Text') {
      const data = (n.data as string).trim();
      if (HAS_LETTERS.test(data)) found.push(data);
    }
    for (const key of Object.keys(n)) {
      if (key === 'parent') continue;
      const v = n[key];
      if (Array.isArray(v)) v.forEach(walk);
      else if (v && typeof v === 'object') walk(v);
    }
  })(ast.fragment);
  return found;
}

/** The root object identifier of a member chain (`copy.a.b` → "copy"). */
function memberRoot(node: Record<string, unknown> | null): string | null {
  let cur = node;
  while (cur && cur.type === 'MemberExpression') {
    cur = cur.object as Record<string, unknown>;
  }
  return cur && cur.type === 'Identifier' ? (cur.name as string) : null;
}

/** Top-level `const/let NAME = copy…` (or destructure) that is not wrapped in
 * $derived: the alias that breaks a locale swap. Nested reads are excluded
 * because they are not top-level statements. */
function nonReactiveAliases(source: string): string[] {
  const ast = parse(source, { modern: true });
  const bad: string[] = [];
  for (const script of [ast.instance, ast.module]) {
    const body = (script?.content as { body?: unknown[] } | undefined)?.body;
    if (!Array.isArray(body)) continue;
    for (const stmt of body as Record<string, unknown>[]) {
      if (stmt.type !== 'VariableDeclaration') continue;
      for (const decl of stmt.declarations as Record<string, unknown>[]) {
        const init = decl.init as Record<string, unknown> | null;
        if (!init) continue;
        const root =
          init.type === 'Identifier'
            ? (init.name as string)
            : init.type === 'MemberExpression'
              ? memberRoot(init)
              : null;
        if (root === 'copy' || root === 'cloudCopy') {
          bad.push(source.slice(stmt.start as number, stmt.end as number).split('\n')[0]);
        }
      }
    }
  }
  return bad;
}

const files = roots.flatMap((r) => svelteFiles(join(repoRoot, r)));

describe('copy guard', () => {
  it('finds components to scan', () => {
    // A path/glob mistake that scans nothing would make every check below
    // pass vacuously.
    expect(files.length).toBeGreaterThan(50);
  });

  it('has no user-facing string literals in markup', () => {
    const offenders: string[] = [];
    for (const file of files) {
      const rel = relative(repoRoot, file).split('\\').join('/');
      const allowed = new Set(ALLOW[rel] ?? []);
      const literals = fragmentStringLiterals(readFileSync(file, 'utf8'));
      for (const lit of literals) {
        if (!allowed.has(lit)) offenders.push(`${rel}: ${JSON.stringify(lit)}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it('never aliases the catalog outside $derived', () => {
    const offenders: string[] = [];
    for (const file of files) {
      const rel = relative(repoRoot, file).split('\\').join('/');
      for (const line of nonReactiveAliases(readFileSync(file, 'utf8'))) {
        offenders.push(`${rel}: ${line.trim()}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it('parses the Svelte AST into the node shape the guard relies on', () => {
    // If a compiler upgrade renames these node types, the walkers above go
    // silent and pass forever. This fails loudly instead.
    const ast = parse(
      `<script>let x = 1;</script><div class="a" title="Hi there">Some text {x}</div>`,
      { modern: true }
    );
    const types = new Set<string>();
    (function walk(node: unknown): void {
      if (!node || typeof node !== 'object') return;
      const n = node as Record<string, unknown>;
      if (typeof n.type === 'string') types.add(n.type as string);
      for (const key of Object.keys(n)) {
        if (key === 'parent') continue;
        const v = n[key];
        if (Array.isArray(v)) v.forEach(walk);
        else if (v && typeof v === 'object') walk(v);
      }
    })(ast.fragment);
    expect(types.has('RegularElement')).toBe(true);
    expect(types.has('Attribute')).toBe(true);
    expect(types.has('Text')).toBe(true);
    expect(ast.instance).toBeTruthy();
  });
});
