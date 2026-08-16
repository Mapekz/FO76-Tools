// Doc-drift guard: the esm-viewer analog of the sibling Rust crates'
// tests/doc_drift.rs (see ../ba2/tests/doc_drift.rs and ../esm/tests/doc_drift.rs).
// Docs are checked against ground truth on disk / in code; failures name the
// exact drifted token and say which side to fix. Skip-lists below start
// EMPTY — a hit means fix the doc (or the code), not add an exception.
//
// Five checks:
//   1. Every path-shaped backtick token in README.md/CLAUDE.md resolves on disk.
//   2. Every `bun run <script>` / `just <recipe>` mentioned in README.md,
//      CLAUDE.md, or the justfile names a real package.json script / justfile
//      recipe; no npm/npx/pnpm invocation is documented as a command to run.
//   3. DESIGN.md's frontmatter `colors:` map and theme.ts's `colors` const
//      agree in both directions (same tokens, same hex, theme.ts is truth).
//   4. No raw hex/rgba color literal exists outside theme.ts.
//   (5. CLAUDE.md's architecture-table paths are covered by check 1's own
//       extractor — see the "architecture table" case in that test.)

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, test } from 'bun:test'

const ROOT = join(import.meta.dir, '..')
const THIS_FILE = join(ROOT, 'src', 'doc-drift.test.ts')

function read(absPath: string): string {
  return readFileSync(absPath, 'utf-8')
}

// ── Known-intentional exceptions — start EMPTY. A hit means fix the doc. ───
const SKIP_PATH_REFS: Set<string> = new Set()
const SKIP_COMMAND_REFS: Set<string> = new Set()
const SKIP_NPM_MENTIONS: Set<string> = new Set()

// ── Markdown code-region extraction (mirrors doc_drift.rs's code_regions) ──
// "Code regions" = fenced code-block bodies plus inline `code span` bodies.
// Restricting extraction to these keeps prose ("or just let predev rebuild
// it") from being misread as a command invocation.

function fencedBlocks(markdown: string): string[] {
  return markdown.split('```').filter((_, i) => i % 2 === 1)
}

function inlineSpans(markdown: string): string[] {
  const prose = markdown
    .split('```')
    .filter((_, i) => i % 2 === 0)
    .join(' ')
  return prose.split('`').filter((_, i) => i % 2 === 1)
}

function codeRegions(markdown: string): string[] {
  return [...fencedBlocks(markdown), ...inlineSpans(markdown)]
}

function stripTrailingPunct(tok: string): string {
  return tok.replace(/[,.;:)\]]+$/, '')
}

// ── Check 1: path references resolve ────────────────────────────────────

// Bare root-level filenames that carry their own extension, e.g. `bun.lock`,
// `.oxlintrc.json`, `package.json`. Deliberately narrow (no `.ts`/`.tsx`) so
// a bare filename mentioned in prose without its directory (e.g. `index.ts`
// in the architecture table's Purpose column) is never mistaken for a
// root-relative path — those mentions are informal, not literal paths.
const ROOT_FILE_RE = /^\.?[\w.-]+\.(json|lock|md|yml|yaml|toml)$/

function stripGlob(pathTok: string): string {
  const star = pathTok.indexOf('*')
  if (star === -1) return pathTok
  const upTo = pathTok.slice(0, star)
  const lastSlash = upTo.lastIndexOf('/')
  return lastSlash === -1 ? '' : upTo.slice(0, lastSlash)
}

function trimSpan(raw: string): string {
  let s = raw.trim()
  if (s.startsWith('"') && s.endsWith('"') && s.length > 1) s = s.slice(1, -1)
  return stripTrailingPunct(s)
}

/** Classifies a trimmed inline-span token as a checkable path reference, or
 * returns null if it doesn't look like one (prose word, identifier, glob
 * with no directory, shell command, etc). */
function classifyPathToken(rawSpan: string): { rel: string; base: string } | null {
  if (/\s/.test(rawSpan)) return null // shell commands / multi-word prose
  const s = trimSpan(rawSpan)
  if (s === '') return null

  const dotdot = s.indexOf('../')
  if (dotdot !== -1) {
    // `rel` keeps its leading `../` and joins onto ROOT (not WORKSPACE) so
    // path.join resolves it exactly once — WORKSPACE is already "one level
    // up"; joining an already-`../`-prefixed rel onto it would pop twice.
    const rel = stripGlob(s.slice(dotdot))
    if (rel === '../') return null
    return { rel, base: ROOT }
  }
  if (s.startsWith('src/')) {
    const rel = stripGlob(s)
    if (rel === '') return null
    return { rel, base: ROOT }
  }
  if (ROOT_FILE_RE.test(s) || s === '.gitignore') {
    return { rel: s, base: ROOT }
  }
  return null
}

test('doc path references resolve (README.md, CLAUDE.md)', () => {
  const docs: [string, string][] = [
    ['README.md', read(join(ROOT, 'README.md'))],
    ['CLAUDE.md', read(join(ROOT, 'CLAUDE.md'))],
  ]

  const failures: string[] = []
  for (const [name, content] of docs) {
    for (const span of inlineSpans(content)) {
      const trimmed = span.trim()
      if (trimmed === '') continue
      const ref = classifyPathToken(span)
      if (!ref) continue
      if (SKIP_PATH_REFS.has(ref.rel)) continue
      const candidate = join(ref.base, ref.rel)
      if (!existsSync(candidate)) {
        failures.push(
          `${name}: \`${trimmed}\` does not exist at ${candidate} — either the doc is stale ` +
            `(fix the path) or this is intentional (add "${ref.rel}" to SKIP_PATH_REFS with a ` +
            `comment explaining why)`,
        )
      }
    }
  }

  expect(failures).toEqual([])
})

// ── Check 2: command references are real ────────────────────────────────

function findBunRunScripts(line: string): string[] {
  const toks = line.trim().split(/\s+/)
  const out: string[] = []
  for (let i = 0; i + 1 < toks.length; i++) {
    if (toks[i] === 'bun' && toks[i + 1] === 'run') {
      const arg = toks[i + 2]
      if (arg && !arg.startsWith('#')) out.push(stripTrailingPunct(arg))
    }
  }
  return out
}

function findJustRecipes(line: string): string[] {
  const toks = line.trim().split(/\s+/)
  const out: string[] = []
  for (let i = 0; i < toks.length; i++) {
    if (toks[i] !== 'just') continue
    const next = toks[i + 1]
    if (!next || next.startsWith('#')) continue // bare `just` == default recipe
    out.push(stripTrailingPunct(next))
  }
  return out
}

function pkgManagerAtLineStart(line: string): string | null {
  const first = line.trim().split(/\s+/)[0]
  return first === 'npm' || first === 'npx' || first === 'pnpm' ? first : null
}

describe('command references are real', () => {
  const packageJson = JSON.parse(read(join(ROOT, 'package.json'))) as {
    scripts: Record<string, string>
  }
  const realScripts = new Set(Object.keys(packageJson.scripts))

  const justfileText = read(join(ROOT, 'justfile'))
  const realRecipes = new Set(
    justfileText
      .split('\n')
      .map((l) => /^([a-z][a-z0-9-]*):/.exec(l)?.[1])
      .filter((r): r is string => Boolean(r)),
  )
  expect(realRecipes.size).toBeGreaterThan(0) // parser sanity — justfile shape changed?

  const readme = read(join(ROOT, 'README.md'))
  const claudeMd = read(join(ROOT, 'CLAUDE.md'))

  test('every `bun run <script>` names a real package.json script', () => {
    const failures: string[] = []
    const sources: [string, string[]][] = [
      ['README.md', codeRegions(readme)],
      ['CLAUDE.md', codeRegions(claudeMd)],
      ['justfile', [justfileText]],
    ]
    for (const [name, regions] of sources) {
      for (const region of regions) {
        for (const line of region.split('\n')) {
          for (const script of findBunRunScripts(line)) {
            if (realScripts.has(script) || SKIP_COMMAND_REFS.has(script)) continue
            failures.push(
              `${name}: \`bun run ${script}\` — no such script in package.json (real scripts: ` +
                `${[...realScripts].toSorted().join(', ')}) — found in: ${line.trim()}`,
            )
          }
        }
      }
    }
    expect(failures).toEqual([])
  })

  test('every `just <recipe>` names a real justfile recipe', () => {
    // Fenced code blocks only (not inline spans): CLAUDE.md's prose also says
    // "run `just gen-types` in `esm/`", a real recipe of the *sibling* esm/
    // repo's justfile, not this one — narrowing to actual invocation blocks
    // avoids mistaking that cross-repo mention for local drift.
    const failures: string[] = []
    const sources: [string, string[]][] = [
      ['README.md', fencedBlocks(readme)],
      ['CLAUDE.md', fencedBlocks(claudeMd)],
    ]
    for (const [name, regions] of sources) {
      for (const region of regions) {
        for (const line of region.split('\n')) {
          for (const recipe of findJustRecipes(line)) {
            if (realRecipes.has(recipe) || SKIP_COMMAND_REFS.has(recipe)) continue
            failures.push(
              `${name}: \`just ${recipe}\` — no such recipe in justfile (real recipes: ` +
                `${[...realRecipes].toSorted().join(', ')}) — found in: ${line.trim()}`,
            )
          }
        }
      }
    }
    expect(failures).toEqual([])
  })

  test('no npm/npx/pnpm invocation is documented as a command to run', () => {
    const failures: string[] = []
    const sources: [string, string[]][] = [
      ['README.md', fencedBlocks(readme)],
      ['CLAUDE.md', fencedBlocks(claudeMd)],
      ['justfile', [justfileText]],
    ]
    for (const [name, regions] of sources) {
      for (const region of regions) {
        for (const line of region.split('\n')) {
          const mgr = pkgManagerAtLineStart(line)
          if (!mgr || SKIP_NPM_MENTIONS.has(mgr)) continue
          failures.push(
            `${name}: \`${line.trim()}\` — this is a bun-only repo (see CLAUDE.md), ` +
              `no ${mgr} command should be documented as something to run`,
          )
        }
      }
    }
    expect(failures).toEqual([])
  })
})

// ── Check 3: theme.ts <-> DESIGN.md palette lockstep ────────────────────

function kebabToCamel(name: string): string {
  return name.replace(/-([a-z0-9])/g, (_, c: string) => c.toUpperCase())
}

function parseDesignColors(designMd: string): Map<string, string> {
  const lines = designMd.split('\n')
  const start = lines.findIndex((l) => l === 'colors:')
  expect(start).toBeGreaterThan(-1) // parser sanity — DESIGN.md frontmatter shape changed?
  const colors = new Map<string, string>()
  const entryRe = /^ {2}([a-z0-9]+(?:-[a-z0-9]+)*): "(#[0-9a-fA-F]{3,8})"$/
  for (let i = start + 1; i < lines.length; i++) {
    const m = entryRe.exec(lines[i]!)
    if (!m) break
    colors.set(m[1]!, m[2]!)
  }
  expect(colors.size).toBeGreaterThan(0) // parser sanity
  return colors
}

function parseThemeColors(themeTs: string): Map<string, string> {
  const lines = themeTs.split('\n')
  const start = lines.findIndex((l) => l === 'export const colors = {')
  expect(start).toBeGreaterThan(-1) // parser sanity — theme.ts shape changed?
  const colors = new Map<string, string>()
  const entryRe = /^\s*([a-zA-Z][a-zA-Z0-9]*): '(#[0-9a-fA-F]{3,8})',$/
  for (let i = start + 1; i < lines.length; i++) {
    const m = entryRe.exec(lines[i]!)
    if (!m) break
    colors.set(m[1]!, m[2]!)
  }
  expect(colors.size).toBeGreaterThan(0) // parser sanity
  return colors
}

test('theme.ts and DESIGN.md color palette are in lockstep', () => {
  const designColors = parseDesignColors(read(join(ROOT, 'DESIGN.md')))
  const themePath = join(ROOT, 'src', 'renderer', 'src', 'theme.ts')
  const themeColors = parseThemeColors(read(themePath))

  const failures: string[] = []

  for (const [kebab, hex] of designColors) {
    const camel = kebabToCamel(kebab)
    const themeHex = themeColors.get(camel)
    if (themeHex === undefined) {
      failures.push(
        `DESIGN.md's \`colors.${kebab}\` (${hex}) has no matching \`${camel}\` key in theme.ts ` +
          `— this is a doc-only token with no implementation; either implement it in theme.ts or ` +
          `remove it from DESIGN.md's frontmatter`,
      )
    } else if (themeHex !== hex) {
      failures.push(
        `DESIGN.md's \`colors.${kebab}\` is "${hex}" but theme.ts's \`${camel}\` is "${themeHex}" ` +
          `— theme.ts is the source of truth — update DESIGN.md's frontmatter to match`,
      )
    }
  }

  for (const [camel, hex] of themeColors) {
    const kebab = camel.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)
    if (!designColors.has(kebab)) {
      failures.push(
        `theme.ts's \`colors.${camel}\` (${hex}) has no matching \`${kebab}\` key in DESIGN.md's ` +
          `frontmatter — theme.ts is the source of truth — update DESIGN.md's frontmatter to match`,
      )
    }
  }

  expect(failures).toEqual([])
})

// ── Check 4: no raw color literals outside theme.ts ─────────────────────

const HEX_COLOR_RE = /#[0-9a-fA-F]{3,8}\b/
const RGBA_RE = /rgba?\(/

function collectSourceFiles(dir: string, skipDirs: Set<string>, skipFiles: Set<string>): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) {
      if (skipDirs.has(full)) continue
      out.push(...collectSourceFiles(full, skipDirs, skipFiles))
    } else if (/\.(ts|tsx)$/.test(entry.name) && !skipFiles.has(full)) {
      out.push(full)
    }
  }
  return out
}

test('no raw color literal exists outside theme.ts', () => {
  const themePath = join(ROOT, 'src', 'renderer', 'src', 'theme.ts')
  const skipDirs = new Set([join(ROOT, 'src', 'shared', 'generated')])
  const skipFiles = new Set([themePath, THIS_FILE])

  const failures: string[] = []
  for (const file of collectSourceFiles(join(ROOT, 'src'), skipDirs, skipFiles)) {
    const lines = read(file).split('\n')
    lines.forEach((line, i) => {
      if (HEX_COLOR_RE.test(line) || RGBA_RE.test(line)) {
        failures.push(
          `${file}:${i + 1}: raw color literal outside theme.ts — add it to theme.ts's ` +
            `\`colors\` const and import it instead: ${line.trim()}`,
        )
      }
    })
  }

  expect(failures).toEqual([])
})
