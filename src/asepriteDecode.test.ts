import assert from "node:assert/strict";
import test from "node:test";
import { deflateSync } from "node:zlib";
import { decodeAseprite } from "./asepriteDecode.ts";

function chunk(type: number, payload: Uint8Array) {
  const bytes = new Uint8Array(6 + payload.length), view = new DataView(bytes.buffer);
  view.setUint32(0, bytes.length, true); view.setUint16(4, type, true); bytes.set(payload, 6); return bytes;
}
function layer(visible = true, opacity = 255, blend = 0) {
  const bytes = new Uint8Array(19), view = new DataView(bytes.buffer);
  view.setUint16(0, visible ? 1 : 0, true); view.setUint16(10, blend, true); bytes[12] = opacity;
  view.setUint16(16, 1, true); bytes[18] = 76; return chunk(0x2004, bytes);
}
function cel(index: number, pixels: number[], x = 0, opacity = 255) {
  const bytes = new Uint8Array(20 + pixels.length), view = new DataView(bytes.buffer);
  view.setUint16(0, index, true); view.setInt16(2, x, true); bytes[6] = opacity;
  view.setUint16(16, pixels.length / 4, true); view.setUint16(18, 1, true); bytes.set(pixels, 20); return chunk(0x2005, bytes);
}
function link(index: number, frame: number, x = 0, opacity = 255) {
  const bytes = new Uint8Array(18), view = new DataView(bytes.buffer);
  view.setUint16(0, index, true); view.setInt16(2, x, true); bytes[6] = opacity;
  view.setUint16(7, 1, true); view.setUint16(16, frame, true); return chunk(0x2005, bytes);
}
function frame(chunks: Uint8Array[], duration = 70) {
  const bytes = new Uint8Array(16 + chunks.reduce((sum, item) => sum + item.length, 0)), view = new DataView(bytes.buffer);
  view.setUint32(0, bytes.length, true); view.setUint16(4, 0xf1fa, true); view.setUint16(6, chunks.length, true); view.setUint16(8, duration, true);
  let cursor = 16; for (const item of chunks) { bytes.set(item, cursor); cursor += item.length; } return bytes;
}
function file(frames: Uint8Array[], width = 2, height = 1) {
  const bytes = new Uint8Array(128 + frames.reduce((sum, item) => sum + item.length, 0)), view = new DataView(bytes.buffer);
  view.setUint32(0, bytes.length, true); view.setUint16(4, 0xa5e0, true); view.setUint16(6, frames.length, true);
  view.setUint16(8, width, true); view.setUint16(10, height, true); view.setUint16(12, 32, true); view.setUint32(14, 1, true); view.setUint16(18, 90, true);
  let cursor = 128; for (const item of frames) { bytes.set(item, cursor); cursor += item.length; } return bytes;
}

const redGreen = [255,0,0,255,0,255,0,255];
test("flat RGBA timelines compose visible normal layers and linked images with their own position and authored timing", async () => {
  const bytes = file([
    frame([layer(), layer(true,191), layer(false), cel(0,redGreen), cel(1,[255,255,255,128],0,170), cel(2,[0,0,255,255])],70),
    frame([link(0,0,-1), link(1,0,1,85)],130), frame([],260),
  ]);
  const original = bytes.slice(); const result = await decodeAseprite(bytes.buffer);
  assert.equal(result.width,2); assert.equal(result.height,1);
  assert.deepEqual(result.frames.map((item) => item.durationMs),[70,130,260]);
  assert.deepEqual(result.frames.map((item) => [...new Uint8Array(item.buffer)]),[
    [255,64,64,255,0,255,0,255], [0,255,0,255,255,255,255,32], [0,0,0,0,0,0,0,0],
  ]);
  assert.deepEqual(bytes,original);
});

function compressedCel(pixels = redGreen, payload = new Uint8Array(deflateSync(Uint8Array.from(pixels)))) {
  const header = cel(0, pixels).subarray(0,26), bytes = new Uint8Array(26 + payload.length), view = new DataView(bytes.buffer);
  bytes.set(header); bytes.set(payload,26); view.setUint32(0,bytes.length,true); view.setUint16(13,2,true); return bytes;
}
test("complete zlib cels yield exact RGBA and positive legacy duration fallback", async () => {
  const result = await decodeAseprite(file([frame([layer(),compressedCel()],0)]).buffer);
  assert.deepEqual([...new Uint8Array(result.frames[0].buffer)],redGreen);
  assert.equal(result.frames[0].durationMs,90);
});

test("compressed output must finish exact length, checksum and EOF before a composite is published", async () => {
  const payload = Uint8Array.from(deflateSync(Uint8Array.from(redGreen)));
  const checksum = payload.slice(); checksum[checksum.length - 1] ^= 1;
  const cases = [
    { cel: compressedCel([255,0,0,255],payload), error: /mehr Pixel/ },
    { cel: compressedCel(redGreen,Uint8Array.from(deflateSync(Uint8Array.from([255,0,0,255])))), error: /unvollständig/ },
    ...[checksum,payload.subarray(0,-1),Uint8Array.from([...payload,7])].map((bytes) => ({cel:compressedCel(redGreen,bytes),error:/Zlib/})),
  ];
  for(const item of cases) await assert.rejects(decodeAseprite(file([frame([layer(),item.cel])]).buffer),item.error);
});

test("unsupported compression runtime fails explicitly while raw previews still work", async (t) => {
  const original = Object.getOwnPropertyDescriptor(globalThis,"DecompressionStream");
  t.after(() => { if(original) Object.defineProperty(globalThis,"DecompressionStream",original); });
  Object.defineProperty(globalThis,"DecompressionStream",{configurable:true,value:class { constructor() { throw new TypeError("deflate unavailable"); } }});
  await assert.rejects(decodeAseprite(file([frame([layer(),compressedCel()])]).buffer),/Dekompression.*verfügbar/);
  assert.deepEqual([...new Uint8Array((await decodeAseprite(file([frame([layer(),cel(0,redGreen)])]).buffer)).frames[0].buffer)],redGreen);
});

test("unqualified rendering semantics, even hidden, are rejected before decoding any cel", async (t) => {
  let started=0;
  const original=Object.getOwnPropertyDescriptor(globalThis,"DecompressionStream");
  t.after(() => { if(original) Object.defineProperty(globalThis,"DecompressionStream",original); });
  Object.defineProperty(globalThis,"DecompressionStream",{configurable:true,value:class { constructor() { started++; throw new Error("should not decode"); } }});
  const badLayer=layer(false,255,1);
  await assert.rejects(decodeAseprite(file([frame([layer(),compressedCel(),badLayer])]).buffer),/Layer|Blend/);
  for(const type of [0x2006,0x2008,0x2023,0xbeef]) await assert.rejects(decodeAseprite(file([frame([layer(),compressedCel(),chunk(type,new Uint8Array(40))])]).buffer),/nicht unterstützt/);
  assert.equal(started,0);
});

test("declared pixel and linked-overdraw work caps reject tiny inputs before deflation or RGBA allocation", async (t) => {
  let started=0; const original=Object.getOwnPropertyDescriptor(globalThis,"DecompressionStream");
  t.after(() => { if(original) Object.defineProperty(globalThis,"DecompressionStream",original); });
  Object.defineProperty(globalThis,"DecompressionStream",{configurable:true,value:class { constructor() { started++; throw new Error("should not decode"); } }});
  const large=compressedCel([0,0,0,0]); new DataView(large.buffer).setUint16(22,2048,true); new DataView(large.buffer).setUint16(24,2048,true);
  await assert.rejects(decodeAseprite(file([frame([layer(),large]),...Array.from({length:16},()=>frame([link(0,0)]))],1,1).buffer),/Arbeitsbudget/);
  const cels=Array.from({length:5},(_,index)=>{const value=large.slice();new DataView(value.buffer).setUint16(6,index,true);return value;});
  await assert.rejects(decodeAseprite(file([frame([...Array.from({length:5},()=>layer()),...cels])],1,1).buffer),/Dekodierte Cel/);
  await assert.rejects(decodeAseprite(file(Array.from({length:5},()=>frame([layer()])),4096,1024).buffer),/Frame-Budget/);
  assert.equal(started,0);
});

test("headers, frame/chunk boundaries, cel uniqueness and backward same-layer links remain strict", async () => {
  const valid=()=>file([frame([layer(),cel(0,redGreen)])]);
  for(const [offset,value,size] of [[0,1,4],[4,0,2],[12,8,2],[8,0,2],[34,2,1],[35,1,1],[128,15,4],[132,0,2],[144,5,4]] as const) {
    const bytes=valid(),view=new DataView(bytes.buffer); if(size===4)view.setUint32(offset,value,true);else if(size===2)view.setUint16(offset,value,true);else bytes[offset]=value;
    if(offset===35)bytes[34]=2; if(offset===34)bytes[35]=1;
    await assert.rejects(decodeAseprite(bytes.buffer),/Aseprite/);
  }
  await assert.rejects(decodeAseprite(file([frame([layer(),link(0,0)])]).buffer),/Verknüpfung/);
  await assert.rejects(decodeAseprite(file([frame([layer(),cel(0,redGreen),cel(0,redGreen)])]).buffer),/mehrere Cels/);
  const noDuration=valid(); new DataView(noDuration.buffer).setUint16(18,0,true); new DataView(noDuration.buffer).setUint16(136,0,true);
  await assert.rejects(decodeAseprite(noDuration.buffer),/Dauer/);
  await assert.rejects(decodeAseprite(valid().subarray(0,-1).slice().buffer),/Dateigröße/);
});
