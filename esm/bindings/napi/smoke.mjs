// N-API smoke test — mirrors the Rust env-gate convention.
// Run with:  FO76_ESM=/path/to/Game.esm bun smoke.mjs
// Without FO76_ESM set: prints SKIP and exits 0 (safe for CI and other devs).
//
// Uses a dynamic import so the env check fires before the native addon is
// resolved — avoids a MODULE_NOT_FOUND error when running without FO76_ESM.

const esmPath = process.env.FO76_ESM
if (!esmPath) {
  console.log('SKIP: set FO76_ESM=/path/to/Game.esm to run the napi smoke test')
  process.exit(0)
}

const { EsmHost } = await import('./index.js')

const host = new EsmHost()
const show = (label, value) => console.log(`${label}:`, JSON.stringify(value).slice(0, 200))
show('open', await host.open(esmPath))
const run = (op) => host.run(esmPath, op)
const groups = await run({ op: 'list_groups' })
console.log('list_groups count:', groups.length)
const weaps = await run({ op: 'list_type_records', sig: 'WEAP', offset: 0, limit: 5 })
show('list_type_records WEAP', weaps)
const first = (rows) => ({ kind: 'auto', value: rows[0].form_id })
if (weaps.length > 0) {
  show('record', await run({ op: 'record', sel: first(weaps), depth: 'stub' }))
  show('walk', await run({ op: 'walk', sel: first(weaps), ref_limit: 20, level: 50, want_refs: false }))
}
const omods = await run({ op: 'list_type_records', sig: 'OMOD', offset: 0, limit: 1 })
if (omods.length > 0) {
  show('chase', await run({ op: 'chase', sel: first(omods), depth: 3, ref_limit: 20 }))
}
const lvlis = await run({ op: 'list_type_records', sig: 'LVLI', offset: 0, limit: 1 })
if (lvlis.length > 0) {
  show(
    'drop_table',
    await run({ op: 'drop_table', sel: first(lvlis), level: 50, max_depth: 8, strict: false }),
  )
}
console.log('SMOKE TEST PASSED')
