import { pathToFileURL } from 'node:url';
const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT));
import assert from 'node:assert/strict';
const browser = await chromium.launch({executablePath: process.env.CHROME, headless:true, args:['--use-fake-device-for-media-stream','--use-fake-ui-for-media-stream']});
try {
 const page = await browser.newPage();
 const errors=[]; page.on('pageerror', e=>errors.push(e.message));
 await page.addInitScript(()=>{
  window.cameraCalls=0; window.cameraTracks=[];
  const original=navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
  navigator.mediaDevices.getUserMedia=async args=>{window.cameraCalls++;const s=await original(args);window.cameraTracks.push(...s.getTracks());return s;};
 });
 await page.route('**/fw/hello.json', async route=>{const response=await route.fetch();const m=await response.json();m.board='waveshare-cam';await route.fulfill({response,json:m});});
 await page.goto(process.env.CAMERA_TEST_URL);
 await page.waitForFunction(()=>document.querySelector('#stat').textContent.includes('insns'),{timeout:30000});
 assert.equal(await page.evaluate(()=>window.cameraCalls),0);
 await page.locator('#camwebcam').click();
 await page.waitForFunction(()=>document.querySelector('#camstat').textContent==='webcam live').catch(async error => {
   console.error(await page.evaluate(()=>({status:document.querySelector('#camstat').textContent, calls:window.cameraCalls,
     videoWidth:document.querySelector('#camvideo').videoWidth, paused:document.querySelector('#camvideo').paused,
     tracks:window.cameraTracks.map(t=>t.readyState)})));
   throw error;
 });
 assert.equal(await page.evaluate(()=>window.cameraCalls),1);
 assert.equal(await page.evaluate(()=>window.cameraTracks[0].readyState),'live');
 await page.locator('#camstop').click();
 assert.equal(await page.evaluate(()=>window.cameraTracks[0].readyState),'ended');
 await page.evaluate(()=>{navigator.mediaDevices.getUserMedia=async()=>{throw new DOMException('test denial','NotAllowedError');};});
 await page.locator('#camwebcam').click();
 await page.waitForFunction(()=>document.querySelector('#camstat').textContent.includes('test denial'));
 assert.equal(await page.locator('#camwebcam').isEnabled(),true);
 assert.deepEqual(errors,[]);
 console.log(JSON.stringify({browser:await browser.version(), noPermissionBeforeClick:true, fakeWebcamStreams:true, stopReleasesTracks:true, denialAllowsRetry:true,pageErrors:errors}));
} finally {await browser.close();}
