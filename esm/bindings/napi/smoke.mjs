import { EsmDatabase } from './index.js';
const db = await EsmDatabase.openDatabase('/home/ankit/dev/fo76/esm-parser/SeventySix.esm');
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
