import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import nodePath from "node:path";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { build } from "vite";
import { expect, it } from "vitest";

it("persists pin offsets and preserves existing settings through the real App and host", async () => {
  const directory = mkdtempSync(nodePath.join(import.meta.dirname, ".ui-regression-test-"));
  try {
    const entry = nodePath.join(directory, "entry.ts");
    writeFileSync(entry, `
      import assert from 'node:assert/strict';
      import { mount, unmount, flushSync } from 'svelte';
      import App from '../App.svelte';
      import { HelperClient, SUPPORTED_HELPER_PROTOCOL } from '../helper-client';
      import { defaultSettings, normalizeSettings } from '../settings-store';
      import { configureHostBridge } from '../host-bridge';

      let receive, receiveStatus;
      let connectCalls = 0, stopCalls = 0, saves = 0, startCalls = 0;
      let failSaves = false;
      const sent = [];
      HelperClient.prototype.onMessage = function(handler) { receive = handler; return () => {}; };
      HelperClient.prototype.onStatus = function(handler) { receiveStatus = handler; return () => {}; };
      HelperClient.prototype.connect = function() { connectCalls++; receiveStatus('connecting'); };
      HelperClient.prototype.disconnect = function() { receiveStatus('disconnected'); };
      HelperClient.prototype.stop = function() { stopCalls++; receiveStatus('disconnected'); };
      HelperClient.prototype.sendConfig = function(settings) { sent.push(structuredClone(settings)); return true; };
      let stored = normalizeSettings(defaultSettings);
      stored.enabled = true;
      stored.hotzonesEnabled = true;
      stored.mouseGestures.enabled = true;
      stored.edgeHide.animationEnabled = false;
      stored.ocr.pinOffset = false;
      stored.taskbarAppearance = { enabled: true, mode: 'acrylic', opacity: 73, tint: '#EAF1FC', showBorder: true };
      const installState = () => ({ installed: true, development: false, version: '0.6.4', bytes: 0 });
      let elevationResult = { ok: true, elevated: true };
      const elevationRequests = [];
      let setElevation = async elevated => { elevationRequests.push(elevated); return elevationResult; };
      configureHostBridge({
        kind: 'desktop', getInitialSettings: () => structuredClone(stored),
        getHelperToken: () => 'test-token', getHelperState: installState,
        getPrivilegeState: () => ({ supported: true, elevated: null }),
        startHelper: async () => { startCalls++; return { ok: true }; },
        setHelperElevation: elevated => setElevation(elevated),
        saveSettings: async value => { if (failSaves) throw new Error("settings-write-failure"); stored = structuredClone(value); saves++; }
      });
      const settle = async () => { for (let i = 0; i < 8; i++) { await Promise.resolve(); flushSync(); } };
      const open = index => { document.querySelectorAll('.mode-nav button')[index].click(); flushSync(); };
      const ready = elevated => { receiveStatus('connected'); receive({ type: 'helper.ready', data: { protocolVersion: SUPPORTED_HELPER_PROTOCOL, version: '0.6.4', elevated } }); flushSync(); };
      const pin = () => document.querySelector('.pin-offset-option button');
      const screenshot = () => { open(3); document.querySelector('button.screenshot').click(); flushSync(); };
      let component = mount(App, { target: document.body });
      flushSync(); await settle();
      screenshot();
      assert.ok(pin(), 'the actual App must expose the pin offset choice');
      assert.equal(pin().textContent.trim(), 'Offset pinned image');
      assert.equal(pin().getAttribute('aria-pressed'), 'false');
      pin().click(); await settle();
      assert.equal(stored.ocr.pinOffset, true, 'pin offset must persist through the actual host');
      document.querySelectorAll('.screenshot-panel .result-modes')[0].querySelectorAll('button')[1].click(); await settle();
      assert.equal(stored.ocr.screenshotResult, 'copy-text');
      assert.equal(pin().disabled, true, 'offset is irrelevant for copy-only output');
      assert.equal(stored.ocr.pinOffset, true, 'copy-only must not discard the last offset choice');
      assert.equal(stored.edgeHide.animationEnabled, false, 'the 0.6.3 animation preference must survive all new settings writes');
      assert.deepEqual(stored.taskbarAppearance, { enabled: true, mode: 'acrylic', opacity: 73, tint: '#EAF1FC', showBorder: true });
      await unmount(component); document.body.replaceChildren();
      component = mount(App, { target: document.body }); flushSync(); await settle(); screenshot();
      assert.equal(pin().getAttribute('aria-pressed'), 'true', 'pin offset must survive remount');
      open(0); receiveStatus('connected'); flushSync();
      const permission = () => document.querySelector('.permission-settings input[role="switch"]');
      assert.ok(permission(), 'both real Apps must expose the supported helper permission control');
      assert.equal(permission().getAttribute('aria-label'), 'Administrator access');
      assert.equal(permission().disabled, true, 'a socket without a ready permission report is still unknown');
      assert.equal(permission().checked, false);
      assert.ok(document.querySelector('.permission-settings').textContent.includes('Permission state unknown'));
      ready(true);
      assert.equal(permission().checked, true);
      assert.equal(permission().disabled, false);
      receiveStatus('disconnected'); flushSync();
      assert.equal(permission().checked, false, 'disconnect must discard the old elevation flag');
      assert.equal(permission().disabled, true);
      assert.ok(document.querySelector('.permission-settings').textContent.includes('Permission state unknown'));
      ready(null);
      assert.equal(permission().disabled, true, 'null from ready is not a known false');
      ready(false);
      assert.equal(permission().checked, false);
      assert.equal(permission().disabled, false, 'actual false from ready is known ordinary mode');
      elevationResult = { ok: false, elevated: true, error: 'retained-helper-switch-failure' };
      const beforeRetainedFailure = connectCalls;
      permission().click(); await settle();
      assert.equal(connectCalls, beforeRetainedFailure + 1, 'a failed switch with known surviving helper state must reconnect');
      ready(true);
      assert.equal(permission().checked, true, 'the reconnected helper ready state remains authoritative');
      assert.equal(document.querySelector('.status-rail').classList.contains('error'), false, 'fresh ready clears the switch error');
      ready(false);
      receive({ type: 'runtime.error', data: { code: 'input_monitor_failed' } }); flushSync();
      assert.equal(document.querySelector('.status-rail').classList.contains('error'), true);
      elevationResult = { ok: true, elevated: false, warning: 'adminCancelled' };
      const beforeCancelledRequest = connectCalls;
      permission().click(); await settle();
      assert.equal(elevationRequests.at(-1), true, 'ordinary mode requests elevation, never guesses from unknown');
      assert.equal(connectCalls, beforeCancelledRequest + 1);
      assert.equal(document.querySelector('.status-rail').classList.contains('error'), false, 'a successful cancellation fallback must not retain an old helper error');
      assert.ok(document.querySelector('.permission-settings').textContent.toLowerCase().includes('cancel'));
      ready(false);
      assert.equal(permission().checked, false);
      receive({ type: 'taskbar.status', data: { state: 'applied', available: true, materials: ['transparent', 'acrylic', 'solid'], backgrounds: 1 } }); flushSync();
      open(4);
      assert.ok(document.querySelector('.drawer').textContent.includes('Taskbar effect applied'), 'new taskbar status must render after permission reconnection');
      assert.deepEqual(sent.at(-1).taskbarAppearance, { enabled: true, mode: 'acrylic', opacity: 73, tint: '#EAF1FC', showBorder: true });
      open(0); ready(true);
      elevationResult = { ok: false, elevated: false, error: 'ordinary-start-failure' };
      permission().click(); await settle();
      assert.equal(permission().checked, false, 'a failed ordinary restart must never retain the old elevated=true UI');
      ready(false);
      elevationResult = { ok: false, elevated: null, error: 'permission-query-failure' };
      permission().click(); await settle();
      assert.equal(permission().checked, false);
      assert.equal(permission().disabled, true);
      assert.ok(document.querySelector('.permission-settings').textContent.includes('Permission state unknown'));
      receiveStatus('connected'); flushSync();
      const unknownRequests = elevationRequests.length;
      permission().dispatchEvent(new Event('change')); await settle();
      assert.equal(elevationRequests.length, unknownRequests, 'unknown must block even a programmatic change event');
      assert.equal(document.querySelector('.master input').disabled, false, 'the master switch remains available for explicit recovery');
      ready(true);
      setElevation = async () => { throw new Error('transport-switch-failure'); };
      permission().click(); await settle();
      assert.equal(permission().disabled, true);
      assert.ok(document.querySelector('.permission-settings').textContent.includes('Permission state unknown'));
      assert.equal(document.querySelector('.master input').disabled, false, 'exception cleanup must release the switching state');
      assert.ok(document.querySelector('.status-rail').textContent.includes('transport-switch-failure'));
      await unmount(component); document.body.replaceChildren();
      localStorage.setItem('convenient-window-language', 'zh-CN');
      component = mount(App, { target: document.body }); flushSync(); await settle(); open(0);
      assert.equal(permission().getAttribute('aria-label'), '管理员权限');
      assert.ok(document.querySelector('.permission-settings').textContent.includes('权限状态未知'));
      ready(false);
      let finishElevation;
      setElevation = elevated => { elevationRequests.push(elevated); return new Promise(resolve => { finishElevation = resolve; }); };
      permission().click(); await settle();
      assert.equal(document.querySelector('.master input').disabled, true, 'no lifecycle toggle is allowed while permission replacement is pending');
      const offButton = [...document.querySelectorAll('.power-actions button')].find(button => button.textContent.includes('关闭功能'));
      assert.equal(offButton.disabled, true);
      const deleteButton = document.querySelector('.power-actions .danger');
      if (deleteButton) assert.equal(deleteButton.disabled, true, 'the host must not delete helper files during a pending elevation request');
      const pendingRequests = elevationRequests.length;
      permission().dispatchEvent(new Event('change')); await settle();
      assert.equal(elevationRequests.length, pendingRequests, 'permission replacement is not reentrant');
      finishElevation({ ok: true, elevated: false, warning: 'adminCancelled' }); await settle();
      assert.equal(document.querySelector('.master input').disabled, false, 'finally releases the lifecycle controls');
      ready(true);
      failSaves = true;
      const beforeFailedSaveRequests = elevationRequests.length;
      permission().click(); await settle();
      assert.equal(elevationRequests.length, beforeFailedSaveRequests, 'settings persistence failure must abort the privilege request');
      assert.equal(permission().checked, true, 'the connected helper remains the same if no switch was attempted');
      assert.equal(permission().disabled, false, 'a failed pre-switch save must not leave the controls stuck');
      failSaves = false;
      const beforeRecovery = startCalls;
      receiveStatus('disconnected'); flushSync();
      await new Promise(resolve => setTimeout(resolve, 1200)); await settle();
      assert.equal(startCalls, beforeRecovery + 1, 'failed pre-switch persistence must preserve eligibility for ordinary connection recovery');
      ready(true);
      await unmount(component);
      console.log('App UI host regression passed');
    `);
    await build({
      configFile: false, logLevel: "silent", plugins: [svelte()],
      build: {
        target: "esnext", minify: false, outDir: nodePath.join(directory, "bundle"),
        lib: { entry, formats: ["es"], fileName: () => "entry.mjs" },
        rollupOptions: { external: ["node:assert/strict"] }
      }
    });
    writeFileSync(nodePath.join(directory, "run.mjs"), `
      import { Window } from 'happy-dom';
      const window = new Window();
      for (const name of ['window', 'document', 'localStorage', 'Node', 'Text', 'Comment', 'Element', 'HTMLElement', 'HTMLInputElement', 'HTMLMediaElement', 'Event', 'KeyboardEvent', 'MutationObserver', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame']) {
        globalThis[name] = name === 'window' ? window : window[name];
      }
      Object.defineProperty(globalThis, 'navigator', { value: window.navigator, configurable: true });
      const animate = window.Element.prototype.animate;
      window.Element.prototype.animate = function(...args) {
        const animation = animate.apply(this, args);
        animation.finished.catch(error => { if (error.name !== 'AbortError') throw error; });
        return animation;
      };
      localStorage.setItem('convenient-window-language', 'en-US');
      try { await import('./bundle/entry.mjs'); }
      finally { await window.happyDOM.close(); }
    `);
    const output = execFileSync(process.execPath, [nodePath.join(directory, "run.mjs")], {
      encoding: "utf8", timeout: 15000, stdio: "pipe"
    });
    expect(output).toContain("App UI host regression passed");
  } finally {
    if (nodePath.dirname(nodePath.resolve(directory)) !== nodePath.resolve(import.meta.dirname)) throw new Error("Unexpected test directory");
    rmSync(directory, { recursive: true, force: true });
  }
}, 30000);
