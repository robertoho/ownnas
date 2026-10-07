// Run: node scripts/test-search-navigation.js
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('web/app.js', 'utf8');
let aborted = 0, renders = 0, resultClick;
const nodes = {
  search: {value:'Report'},
  'search-panel': {hidden:false},
  'search-results': {innerHTML:'matches', addEventListener:(_,handler)=>resultClick=handler},
  'zip-link': {}, 'empty-trash-btn': {},
};
const context = {
  loadGen:0, state:{path:'',filter:'Report',searchHits:[{path:'Reports',dir:true}],entries:[],me:{readonly:false},selected:new Set()},
  $:id=>nodes[id], observeModelThumbnails(){}, renderFiles(){renders++}, renderCrumbs(){},syncUndo(){},syncBookmarkBtn(){},scheduleFolderSizes(){},closePreview(){},inTrashPath:()=>false,
  document:{querySelector:()=>({scrollTop:42})}, window:{confirm:()=>false},
  api:async()=>({path:'Reports',entries:[{name:'AnnualReport.docx',dir:false}]}),
  closeLibrary(){},closeSettings(){},pathToHash:path=>'#/'+path,location:{hash:'#/Reports'},toast(){},
};
vm.createContext(context);
vm.runInContext(source.slice(source.indexOf('function resetSearch()'),source.indexOf('function syncSearchHint()')),context);
vm.runInContext(source.slice(source.indexOf('async function load('),source.indexOf('function applyFolderSize(')),context);
vm.runInContext(source.slice(source.indexOf('async function go('),source.indexOf('function findEntry(')),context);
function searching() {
  context.state.filter='Report';nodes.search.value='Report';nodes['search-panel'].hidden=false;
  nodes['search-results'].innerHTML='matches';context.state.searchHits=[{path:'Reports/AnnualReport.docx',name:'AnnualReport.docx',dir:false}];
  context.state.searchController={abort(){aborted++}};
}
(async()=>{
 searching(); await context.load('Reports');
 assert.equal(context.state.filter,'');assert.equal(nodes.search.value,'');assert.equal(nodes['search-panel'].hidden,true);assert.equal(context.state.searchHits.length,0);assert.equal(aborted,1);
 searching();await context.load('Reports',{keepPreview:true});assert.equal(nodes.search.value,'Report','refresh preserves search');
 await context.go('Reports');assert.equal(nodes.search.value,'','entering current folder also clears search');
 searching();context.state.editor={dirty:true};await context.load('Other');assert.equal(nodes.search.value,'Report','cancelled navigation preserves search');context.state.editor=null;
 const api=context.api;context.api=async()=>{throw Error('offline')};await assert.rejects(context.load('Other'));assert.equal(nodes.search.value,'Report','failed navigation preserves search');context.api=api;
 // Result selection clears the input/filter before opening the file or navigating.
 let opened, navigated;
 context.openEntry=async entry=>{opened=entry;assert.equal(nodes.search.value,'');};context.go=async path=>{navigated=path;assert.equal(nodes.search.value,'');};
 const start=source.indexOf('$("search-results").addEventListener("click"');
 vm.runInContext(source.slice(start,source.indexOf('$("activity-btn")',start)),context);
 searching();resultClick({target:{closest:()=>({dataset:{hit:'Reports/AnnualReport.docx'}})}});
 assert.equal(opened.path,'Reports/AnnualReport.docx');assert.equal(context.state.filter,'');assert.equal(nodes['search-results'].innerHTML,'');
 searching();context.state.searchHits=[{path:'Reports',dir:true}];resultClick({target:{closest:()=>({dataset:{hit:'Reports'}})}});assert.equal(navigated,'Reports');
 // The visible list matches partial names in the middle, independent of case.
 context.state.entries=[{name:'AnnualReport.DOCX',dir:false,tags:[]},{name:'MyProjectsArchive',dir:true,tags:[]}];context.state.sort='name';context.state.direction='asc';
 vm.runInContext(source.slice(source.indexOf('function sortedEntries()'),source.indexOf('function fileTypeLabel(')),context);
 context.state.filter='ualREP';assert.equal(context.sortedEntries()[0].name,'AnnualReport.DOCX');
 context.state.filter='PROJECTS';assert.equal(context.sortedEntries()[0].name,'MyProjectsArchive');
 assert.ok(renders>0);
 console.log('PASS: navigation/result search reset, cancelled/failed navigation, refresh preservation, and partial file/folder names.');
})().catch(err=>{console.error(err);process.exitCode=1});
