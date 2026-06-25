// N-API smoke test — mirrors the Rust #[ignore] env-gate convention.
// Run with:  FO76_ESM=/path/to/SeventySix.esm npm test
// Without FO76_ESM set: prints SKIP and exits 0 (safe for CI and other devs).
//
// Uses a dynamic import so the env check fires before the native addon is
// resolved — avoids a MODULE_NOT_FOUND error when running without FO76_ESM.

const esmPath = process.env.FO76_ESM;
if (!esmPath) {
  console.log('SKIP: set FO76_ESM=/path/to/SeventySix.esm to run the napi smoke test');
  process.exit(0);
}

// Only load the native addon when we actually intend to run.
const { EsmDatabase } = await import('./index.js');

const db = await EsmDatabase.openDatabase(esmPath);
const info = db.fileInfo();
console.log('fileInfo:', JSON.stringify(info).slice(0, 200));
const groups = db.listGroups();
console.log('listGroups count:', Array.isArray(groups) ? groups.length : '?');
const weaps = db.listTypeRecords('WEAP', 0, 5);
console.log('listTypeRecords WEAP:', JSON.stringify(weaps).slice(0, 300));
if (weaps.length > 0) {
  const rec = db.recordByFormid(weaps[0].form_id, 'stub');
  console.log('recordByFormid:', JSON.stringify(rec).slice(0, 200));
}
console.log('SMOKE TEST PASSED');
