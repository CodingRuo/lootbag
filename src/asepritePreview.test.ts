import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { imagePreviewUrl, loadAsepriteAnimation, decodeAsepriteUrl } from "./imagePreviewUrl.ts";
import { decodeAseprite } from "./asepriteDecode.ts";
import { ImageTexturePreviewLoader } from "./TgaPreviewLoader.ts";
import { LoadingManager, Mesh, MeshStandardMaterial, DataTexture } from "three";
import { loadModel } from "./loadModel.ts";

// Owned tiny 2x1 RGBA source, compressed red/green then linked with its own negative X.
const source = Uint8Array.from(Buffer.from("+AAAAOClAgACAAEAIAABAAAAWgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABQAAAA+vECAEYAAAAAAAAAGQAAAAQgAQAAAAAAAAAAAAAA/wAAAAEATCcAAAAFIAAAAAAAAP8CAAAAAAAAAAACAAEAeJz7z8DwHwQBEPcD/SgAAAD68QEAggAAAAAAAAAYAAAABSAAAP//AAD/AQAAAAAAAAAAAAA=","base64"));
const transport = (bytes = source) => `data:application/octet-stream;base64,${Buffer.from(bytes).toString("base64")}#snapshot=owned`;
function boundary(t: TestContext, stall: "worker" | "png" | null = null) {
  const saved = ["Worker","document","ImageData"].map(name=>[name,Object.getOwnPropertyDescriptor(globalThis,name)] as const);
  t.after(()=>{for(const [name,descriptor] of saved){if(descriptor)Object.defineProperty(globalThis,name,descriptor);else Reflect.deleteProperty(globalThis,name);}});
  let started!:()=>void, encoding!:()=>void;
  const firstPost=new Promise<void>(resolve=>{started=resolve;}),firstPng=new Promise<void>(resolve=>{encoding=resolve;});
  let jobs=0,active=0,maxActive=0;
  class WorkerBoundary {
    onmessage:((event:{data:unknown})=>void)|null=null;onerror:(()=>void)|null=null;onmessageerror:(()=>void)|null=null;stopped=false;
    constructor(url:URL){assert.match(url.pathname,/asepriteWorker\.ts$/);jobs++;active++;maxActive=Math.max(maxActive,active);}
    postMessage(message:ArrayBuffer|{buffer:ArrayBuffer;animation:boolean}){
      started();if(stall==="worker"&&jobs===1)return;
      const animation=!(message instanceof ArrayBuffer),buffer=animation?message.buffer:message;
      void decodeAseprite(buffer,undefined,!animation).then(result=>{if(!this.stopped)this.onmessage?.({data:animation?result:{width:result.width,height:result.height,buffer:result.frames[0].buffer}});},reason=>{if(!this.stopped)this.onmessage?.({data:{error:(reason as Error).message}});});
    }
    terminate(){if(!this.stopped){this.stopped=true;active--;}}
  }
  const pixels:number[][]=[],canvases:{width:number;height:number}[]=[],sizes:number[][]=[];
  Object.defineProperty(globalThis,"Worker",{configurable:true,value:WorkerBoundary});
  Object.defineProperty(globalThis,"ImageData",{configurable:true,value:class{constructor(data:Uint8ClampedArray){pixels.push([...data]);}}});
  Object.defineProperty(globalThis,"document",{configurable:true,value:{createElement:()=>{const canvas={width:0,height:0,getContext:()=>({putImageData:()=>{},drawImage:()=>{}}),toBlob:(accept:(blob:Blob)=>void)=>{sizes.push([canvas.width,canvas.height]);encoding();if(stall!=="png"||sizes.length>1)accept(new Blob(["owned PNG"],{type:"image/png"}));}};canvases.push(canvas);return canvas;}}});
  return{firstPost,firstPng,pixels,canvases,sizes,counts:()=>({jobs,active,maxActive})};
}

test("Aseprite animation owns complete timed frames and static aliases use only the first composite",async(t)=>{
  const native=boundary(t),controller=new AbortController();
  const animation=await loadAsepriteAnimation(transport(),controller.signal);
  assert.deepEqual(animation.frames.map(frame=>frame.durationMs),[70,130]);
  assert.deepEqual(animation.frames.map(frame=>[...new Uint8Array(frame.buffer)]),[[255,0,0,255,0,255,0,255],[0,255,0,255,0,0,0,0]]);
  controller.abort();assert.equal(animation.frames.length,0);
  for(const filename of ["Owned.ASE","Owned.aseprite"]){
    const preview=await imagePreviewUrl(transport(),filename,new AbortController().signal,1);
    assert.equal((await fetch(preview.url)).headers.get("content-type"),"image/png");preview.release();await assert.rejects(fetch(preview.url));
  }
  assert.deepEqual(native.pixels,[[255,0,0,255,0,255,0,255],[255,0,0,255,0,255,0,255]]);
  assert.deepEqual(native.sizes,[[1,1],[1,1]]);
  assert.ok(native.canvases.every(canvas=>canvas.width===0&&canvas.height===0));
  const first=await decodeAsepriteUrl(transport(),new AbortController().signal);assert.deepEqual([...new Uint8Array(first.buffer)],[255,0,0,255,0,255,0,255]);
  assert.deepEqual(new Uint8Array(await(await fetch(transport())).arrayBuffer()),source);
  assert.equal(native.counts().active,0);assert.equal(native.counts().maxActive,1);
});

test("external Aseprite model textures keep snapshot transport, temporary path and balanced manager completion",async(t)=>{
  boundary(t); const manager=new LoadingManager(),resolved:string[]=[];let errors=0;
  manager.setURLModifier(url=>{resolved.push(url);return transport();});manager.onError=()=>errors++;
  const complete=new Promise<void>(resolve=>{manager.onLoad=resolve;});
  const loader=new ImageTexturePreviewLoader(manager,new AbortController().signal,"ASEPRITE");
  loader.setPath("https://model.invalid/owned/Textures/"); const texture=loader.load("Owned.ase");loader.setPath("");
  await complete;assert.equal(errors,0);assert.deepEqual(resolved,["https://model.invalid/owned/Textures/Owned.ase"]);
  assert.deepEqual([...texture.image.data!],[255,0,0,255,0,255,0,255]);texture.dispose();
});

test("Aseprite worker/PNG cancellation and timeout release the serial owner for a valid source",async(t)=>{
  for(const stall of ["worker","png"] as const)await t.test(stall,async(child)=>{
    const native=boundary(child,stall),controller=new AbortController();
    const pending=imagePreviewUrl(transport(),"Owned.ase",controller.signal),failure=assert.rejects(pending,{name:"AbortError"});
    await(stall==="worker"?native.firstPost:native.firstPng);
    const next=imagePreviewUrl(transport(),"Next.aseprite",new AbortController().signal);
    controller.abort();await failure;const valid=await next;valid.release();
    assert.deepEqual(native.counts(),{jobs:2,active:0,maxActive:1});assert.ok(native.canvases.every(canvas=>canvas.width===0&&canvas.height===0));
  });
  await t.test("timeout",async(child)=>{
    child.mock.timers.enable({apis:["setTimeout"]});const native=boundary(child,"worker");
    const pending=loadAsepriteAnimation(transport(),new AbortController().signal),failure=assert.rejects(pending,/Zeitlimit/);
    await native.firstPost;child.mock.timers.tick(30000);await failure;
    (await loadAsepriteAnimation(transport(),new AbortController().signal)).release();assert.deepEqual(native.counts(),{jobs:2,active:0,maxActive:1});
  });
});

test("malformed timelines fail before publishing and do not strand queued valid frames",async(t)=>{
  const native=boundary(t);
  const broken=loadAsepriteAnimation(transport(source.subarray(0,-1)),new AbortController().signal),failure=assert.rejects(broken,/Dateigröße/);
  const next=loadAsepriteAnimation(transport(),new AbortController().signal);await failure;
  const valid=await next;assert.equal(valid.frames.length,2);valid.release();assert.equal(valid.frames.length,0);
  assert.deepEqual(native.counts(),{jobs:2,active:0,maxActive:1});
});

test("oversize Aseprite streams cancel before any worker or native image decoding",async(t)=>{
  const native=boundary(t);let cancelled=false;
  t.mock.method(globalThis,"fetch",async()=>new Response(new ReadableStream({cancel(){cancelled=true;}}),{headers:{"content-length":"33554433"}}));
  await assert.rejects(loadAsepriteAnimation("asset://localhost/large.ase",new AbortController().signal),/32 MiB/);
  assert.equal(cancelled,true);assert.equal(native.counts().jobs,0);
});

test("external Aseprite-textured OBJ preserves exact first-composite pixels and complete original-bundle eligibility",async(t)=>{
  boundary(t);
  const saved=["window","ProgressEvent"].map(name=>[name,Object.getOwnPropertyDescriptor(globalThis,name)] as const);
  t.after(()=>{for(const[name,descriptor]of saved){if(descriptor)Object.defineProperty(globalThis,name,descriptor);else Reflect.deleteProperty(globalThis,name);}});
  const originals:Record<string,string>={"Owned.obj":"mtllib Owned.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Owned\nf 1 2 3\n","Owned.mtl":"newmtl Owned\nmap_Kd Textures/Owned.ase\n"};
  Object.defineProperty(globalThis,"ProgressEvent",{configurable:true,value:class{constructor(_name:string,_options:unknown){}}});
  Object.defineProperty(globalThis,"window",{configurable:true,value:{__TAURI_INTERNALS__:{convertFileSrc:(path:string)=>path.endsWith(".ase")?transport():`data:text/plain;base64,${Buffer.from(originals[path]).toString("base64")}`}}});
  const snapshot={snapshotId:"owned-aseprite",fileId:1,format:"OBJ",entryPath:"Owned.obj",declaredPaths:["Owned.mtl","Textures/Owned.ase"],errors:[],resources:["Owned.obj","Owned.mtl","Textures/Owned.ase"].map(path=>({relativePath:path,path,sizeBytes:1}))};
  const result=await loadModel(snapshot,new AbortController().signal);
  assert.deepEqual(result.warnings,[]);assert.deepEqual(result.replacements,[]);assert.equal(result.staticAsepriteTextures,true);
  assert.deepEqual(result.resourcePaths.sort(),["Owned.mtl","Owned.obj","Textures/Owned.ase"]);
  const mesh=result.model.children[0]as Mesh,texture=(mesh.material as MeshStandardMaterial).map as DataTexture;
  assert.deepEqual([...texture.image.data!],[255,0,0,255,0,255,0,255]);result.dispose();
});


test("Aseprite texture errors and cancellation balance manager events and permit the next valid load",async(t)=>{
  const native=boundary(t);
  for(const mode of ["malformed","snapshot","source","manager"]){
    const manager=new LoadingManager(),controller=new AbortController(),events:string[]=[];
    const complete=new Promise<void>(resolve=>{manager.onLoad=()=>{events.push("complete");resolve();};});
    manager.onStart=()=>events.push("start");manager.onError=()=>events.push("error");
    manager.setURLModifier(()=>{if(mode==="snapshot")throw new Error("Outside stamped package");return transport(mode==="malformed"?source.subarray(0,-1):source);});
    const texture=new ImageTexturePreviewLoader(manager,controller.signal,"ASEPRITE").load("Owned.ase",()=>assert.fail("Failed source must not publish pixels"),undefined,()=>events.push("callback"));
    if(mode==="source")controller.abort();else if(mode==="manager")manager.abort();
    await complete;assert.deepEqual(events,["start","error","callback","complete"]);assert.equal(texture.image.data,null);texture.dispose();
  }
  const manager=new LoadingManager();manager.setURLModifier(()=>transport());
  const complete=new Promise<void>(resolve=>{manager.onLoad=resolve;});
  const texture=new ImageTexturePreviewLoader(manager,new AbortController().signal,"ASEPRITE").load("Valid.aseprite");
  await complete;assert.deepEqual([...texture.image.data!],[255,0,0,255,0,255,0,255]);texture.dispose();
  assert.equal(native.counts().active,0);assert.equal(native.counts().maxActive,1);
});
