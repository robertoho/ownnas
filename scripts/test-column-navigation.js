const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('web/app.js', 'utf8');
const host = {innerHTML: '', lastElementChild: {scrollIntoView() {}}};
const root = [{path:'Projects',name:'Projects',kind:'folder',dir:true,folderAppearance:{color:'#a855f7',icon:'star'}}];
const children = [{path:'Projects/Models',name:'Models',kind:'folder',dir:true}];
const files = [{path:'Projects/Models/part.stl',name:'part.stl',kind:'model3d',dir:false}];
const requests = [], messages = [];
const context = {columnGeneration:0,columnEntries:new Map(),state:{path:'Projects/Models',view:'columns',entries:files,me:{rootName:'Library'}},
  $:()=>host, esc:String, typeIcon:()=>'<svg/>', entryIcon:()=>'<svg/>', sortedEntries:()=>files,
  api:async url=>{requests.push(url);return {entries:url.includes('Projects')?children:root};},toast:message=>messages.push(message)};
vm.createContext(context);
vm.runInContext(source.slice(source.indexOf("const FOLDER_ICONS ="), source.indexOf("function sortedEntries(")), context);
vm.runInContext(source.slice(source.indexOf('async function renderColumns()'),source.indexOf('async function api(')),context);
(async()=>{
 await context.renderColumns();
 assert.equal((host.innerHTML.match(/class="finder-column"/g)||[]).length,3);
 assert.ok(host.innerHTML.includes('part.stl'));
 assert.ok(host.innerHTML.includes('color:#a855f7'), 'column view displays folder appearance');
 assert.equal(context.columnEntries.size,3);
 assert.equal(requests.length,2,'current column reuses existing listing');
 // A navigation that finishes later must never replace the current columns.
 let resolve;
 context.api=()=>new Promise(r=>{resolve=r});
 context.state.path='Projects';
 const pending=context.renderColumns();
 context.state.path='Elsewhere';resolve({entries:root});await pending;
 assert.ok(host.innerHTML.includes('part.stl'));
 context.undoHistory=[{path:'moved.txt',dest:'original.txt',label:'move'}]; context.undoBusy=false;
 context.syncUndo=()=>{};context.invalidateFolderSizes=()=>{};context.load=async()=>{};
 vm.runInContext(source.slice(source.indexOf('async function undoFileOperation()'),source.indexOf('async function renderColumns()')),context);
 context.api=async()=>{throw Error('original location occupied')};
 await context.undoFileOperation();assert.equal(context.undoHistory.length,1);assert.equal(context.undoBusy,false);
 context.api=async(url,opts)=>{assert.equal(url,'/api/undo-move');assert.equal(opts.json.dest,'original.txt');};
 await context.undoFileOperation();assert.equal(context.undoHistory.length,0);
 console.log('PASS: depth columns, stale navigation protection, and undo conflict retry.');
})().catch(err=>{console.error(err);process.exitCode=1});
