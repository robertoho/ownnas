// Exercise real model loaders/camera fitting with a renderer stub (no GPU required).
// Run: node --experimental-vm-modules scripts/test-model-thumbnails.mjs
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';
const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const realThree = await import(pathToFileURL(path.join(root, 'web/vendor/three/three.module.min.js')));
class XmlNode {
  constructor(name, attributes = {}) {
    this.nodeName = name;
    this.attributes = Object.entries(attributes).map(([name, value]) => ({ name, value }));
    this.children = [];
    this.textContent = '';
  }
  getAttribute(name) { return this.attributes.find(attribute => attribute.name === name)?.value ?? null; }
  descendants() { return this.children.flatMap(child => [child, ...child.descendants()]); }
  querySelectorAll(selector) {
    let nodes = [this];
    for (const part of selector.split(/\s+/)) nodes = nodes.flatMap(node => node.descendants().filter(child => child.nodeName === part));
    return nodes;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
}
class MiniDOMParser {
  parseFromString(xml) {
    const stack = [];
    let rootNode = null;
    for (const token of String(xml).match(/<!--[\s\S]*?-->|<[^>]+>|[^<]+/g) || []) {
      if (token.startsWith('<!--') || token.startsWith('<?') || token.startsWith('<!')) continue;
      if (token.startsWith('</')) { stack.pop(); continue; }
      if (token.startsWith('<')) {
        const selfClosing = /\/>$/.test(token);
        const inside = token.slice(1, selfClosing ? -2 : -1).trim();
        const match = inside.match(/^([^\s/>]+)/);
        if (!match) continue;
        const name = match[1], attributes = {};
        const attrText = inside.slice(name.length);
        for (const attr of attrText.matchAll(/([^\s=]+)\s*=\s*(['"])(.*?)\2/g)) attributes[attr[1]] = attr[3];
        const node = new XmlNode(name, attributes);
        if (stack.length) stack.at(-1).children.push(node); else rootNode = node;
        if (!selfClosing) stack.push(node);
      } else if (stack.length) {
        stack.at(-1).textContent += token;
      }
    }
    return {
      documentElement: rootNode,
      querySelectorAll(selector) {
        const found = rootNode?.querySelectorAll(selector) || [];
        return rootNode?.nodeName === selector ? [rootNode, ...found] : found;
      },
      querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
    };
  }
}
const fixtures = new Map();
const vertices = [[0,0,0],[1,0,0],[0,1,0],[0,0,1]];
const faces = [[0,2,1],[0,1,3],[0,3,2],[1,2,3]];
fixtures.set('shape.stl', 'solid shape\n' + faces.map(f => 'facet normal 0 0 0\nouter loop\n' + f.map(i => 'vertex ' + vertices[i].join(' ') + '\n').join('') + 'endloop\nendfacet\n').join('') + 'endsolid shape\n');
fixtures.set('shape.obj', vertices.map(v => 'v '+v.join(' ')+'\n').join('') + faces.map(f => 'f '+f.map(i=>i+1).join(' ')+'\n').join(''));
fixtures.set('shape.ply', 'ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nelement face 4\nproperty list uchar int vertex_indices\nend_header\n' + vertices.map(v=>v.join(' ')+'\n').join('') + faces.map(f=>'3 '+f.join(' ')+'\n').join(''));
const positions = new Float32Array([-1,0,0, 1,0,0, 0,1,0]);
const bytes = Buffer.from(positions.buffer);
const gltf = {asset:{version:'2.0'},scene:0,scenes:[{nodes:[0]}],nodes:[{mesh:0}],meshes:[{primitives:[{attributes:{POSITION:0}}]}],buffers:[{uri:'shape.bin',byteLength:bytes.length}],bufferViews:[{buffer:0,byteOffset:0,byteLength:bytes.length}],accessors:[{bufferView:0,componentType:5126,count:3,type:'VEC3',min:[-1,0,0],max:[1,1,0]}]};
fixtures.set('models/shape.gltf', JSON.stringify(gltf)); fixtures.set('models/shape.bin', bytes);
const inline = structuredClone(gltf); inline.buffers[0].uri = 'data:application/octet-stream;base64,' + bytes.toString('base64'); fixtures.set('inline.gltf', JSON.stringify(inline));
const binary = structuredClone(gltf); delete binary.buffers[0].uri;
let json = Buffer.from(JSON.stringify(binary)); json = Buffer.concat([json,Buffer.alloc((4-json.length%4)%4,0x20)]);
const glb = Buffer.alloc(12+8+json.length+8+bytes.length); glb.writeUInt32LE(0x46546c67,0);glb.writeUInt32LE(2,4);glb.writeUInt32LE(glb.length,8);glb.writeUInt32LE(json.length,12);glb.writeUInt32LE(0x4e4f534a,16);json.copy(glb,20);const offset=20+json.length;glb.writeUInt32LE(bytes.length,offset);glb.writeUInt32LE(0x004e4942,offset+4);bytes.copy(glb,offset+8);fixtures.set('shape.glb',glb);
const external=structuredClone(gltf);external.buffers[0].uri='https://external.invalid/model.bin';fixtures.set('external.gltf',JSON.stringify(external));
const metallic = structuredClone(inline);
metallic.materials = [{pbrMetallicRoughness:{baseColorFactor:[0,0,0,1],metallicFactor:1,roughnessFactor:0}}];
metallic.meshes[0].primitives[0].material = 0;
fixtures.set('black-metal.gltf', JSON.stringify(metallic));
fixtures.set('tiny.obj', vertices.map(v => 'v '+v.map(x=>x*1e-8).join(' ')+'\n').join('') + faces.map(f => 'f '+f.map(i=>i+1).join(' ')+'\n').join(''));
fixtures.set('huge.obj', vertices.map(v => 'v '+v.map(x=>x*1e8).join(' ')+'\n').join('') + faces.map(f => 'f '+f.map(i=>i+1).join(' ')+'\n').join(''));
let rendered=0, disposed=0, expect3mfOrientation=false;
class Renderer {
  domElement={toBlob: cb=>cb(new Blob(['test-capture'],{type:'image/png'}))};
  setSize(width,height){assert.equal(width,480);assert.equal(height,480);}
  render(scene,camera){
    const box=new realThree.Box3().setFromObject(scene);
    assert.ok(!box.isEmpty());
    const center=box.getCenter(new realThree.Vector3());assert.ok(center.length()<1e-5,'model must be centered');
    assert.ok(camera.near>0 && camera.far>camera.near);
    const size=box.getSize(new realThree.Vector3());
    assert.ok(Math.abs(Math.max(size.x,size.y,size.z)-2)<1e-5,'model units must be normalized');
    if(expect3mfOrientation){
      assert.ok(size.y>size.x*3.5 && size.y>size.z*3.5,'3MF Z-up coordinates must be converted to Y-up');
      expect3mfOrientation=false;
    }
    assert.ok(camera.position.distanceTo(center)>camera.near,'tiny models must not be clipped');
    assert.equal(this.toneMapping,realThree.ACESFilmicToneMapping);
    scene.traverse(object=>{
      if(!object.isMesh)return;
      assert.equal(object.material.metalness,0,'metal must not depend on absent environment reflections');
      assert.equal(object.material.side,realThree.DoubleSide);
      assert.ok(object.material.color.r>.3,'black source material must become visible clay');
      const normals=object.geometry.getAttribute('normal');
      assert.ok(normals,'missing normals must be generated');
      for(let i=0;i<normals.count;i++) assert.ok(Math.hypot(normals.getX(i),normals.getY(i),normals.getZ(i))>.5,'zero STL normals must be repaired');
    });
    const direction=camera.getWorldDirection(new realThree.Vector3());
    assert.ok(direction.dot(camera.position.clone().negate().normalize())>.99,'camera must face model');
    rendered++;
  }
  dispose(){disposed++;} forceContextLoss(){}
}
const nativeFetch = globalThis.fetch;
const NativeRequest = globalThis.Request;
globalThis.Request = class extends NativeRequest { constructor(input, options) { super(typeof input === 'string' ? new URL(input, 'http://localhost/').href : input, options); } };
const context=vm.createContext({console,setTimeout,clearTimeout,Blob,URL,TextDecoder,TextEncoder,ArrayBuffer,Uint8Array,Float32Array,Headers,Request,Response,
  DOMParser:MiniDOMParser,
  location:{href:'http://localhost/',origin:'http://localhost'},
  ProgressEvent:class {constructor(type,opts){Object.assign(this,{type},opts);}},
  fetch:async (request)=>{
    const url=new URL(typeof request==='string'?request:request.url,'http://localhost/');
    if(url.protocol==='data:')return nativeFetch(url);
    assert.equal(url.origin,'http://localhost','no external requests');
    const data=fixtures.get(url.searchParams.get('path'));
    return data===undefined?new Response('',{status:404}):new Response(data);
  },
});
context.self=context;
globalThis.fetch = context.fetch;
globalThis.ProgressEvent = context.ProgressEvent;
const namespace={...realThree,WebGLRenderer:Renderer};
const three=new vm.SyntheticModule(Object.keys(namespace),function(){for(const [key,value]of Object.entries(namespace))this.setExport(key,value);},{context});
const modules=new Map();
async function getModule(filename){
  if(filename.endsWith('three.module.min.js'))return three;
  if(modules.has(filename))return modules.get(filename);
  const module=new vm.SourceTextModule(await fs.readFile(filename,'utf8'),{context,identifier:filename});modules.set(filename,module);return module;
}
async function linker(specifier,parent){
  const filename=specifier.startsWith('/assets/')?path.join(root,'web',specifier.slice('/assets/'.length)):path.resolve(path.dirname(parent.identifier),specifier);
  return getModule(filename);
}
const fflate=await getModule(path.join(root,'web/vendor/three/addons/libs/fflate.module.js'));
await fflate.link(linker);await fflate.evaluate();
const modelXml='<?xml version="1.0"?><model unit="millimeter"><resources><object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="4"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="2" v3="3"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>';
const relsXml='<?xml version="1.0"?><Relationships><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>';
fixtures.set('shape.3mf',fflate.namespace.zipSync({
  '_rels/.rels':new TextEncoder().encode(relsXml),
  '3D/3dmodel.model':new TextEncoder().encode(modelXml),
}));
const model=await getModule(path.join(root,'web/model-viewer.js'));await model.link(linker);await model.evaluate();
for(const name of ['shape.stl','shape.obj','shape.ply','models/shape.gltf','inline.gltf','shape.glb','black-metal.gltf','tiny.obj','huge.obj']){
  const blob=await model.namespace.renderModelThumbnail({url:'/api/raw?path='+encodeURIComponent(name),name:name.split('/').pop(),size:1000});
  assert.equal(blob.type,'image/png');
}
expect3mfOrientation=true;
const threeMfBlob=await model.namespace.renderModelThumbnail({url:'/api/raw?path=shape.3mf',name:'shape.3mf',size:1000});
assert.equal(threeMfBlob.type,'image/png');
assert.equal(expect3mfOrientation,false);
assert.equal(rendered,10);assert.equal(disposed,10);
await assert.rejects(model.namespace.renderModelThumbnail({url:'/api/raw?path=shape.stl',name:'shape.stl',size:81*1024*1024}),/80 MB/);
await assert.rejects(model.namespace.renderModelThumbnail({url:'/api/raw?path=external.gltf',name:'external.gltf',size:1000}),/External model resources/);
assert.equal(disposed,10,'rejected models must not allocate a renderer');
console.log('PASS: STL, OBJ, PLY, GLTF, GLB, 3MF orientation, camera framing, disposal, size limit, and external-resource rejection. GPU output is not tested.');
