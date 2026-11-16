// NAPI coverage for the raw WebKitSession surface
// (browserContext.newWebKitSession): send reaches the live target,
// events stream in wire order, a closed page answers nothing, and
// Chromium refuses it.

import { describe, it, expect, beforeAll, afterAll } from "bun:test";
import { type Browser, type WebKitEvents } from "../index.js";
import { launchForBackend } from "./_helpers.js";

const BACKENDS: string[] = process.env.FERRIDRIVER_BACKEND
  ? [process.env.FERRIDRIVER_BACKEND]
  : ["cdp-pipe", "webkit"];

async function nextNamed(events: WebKitEvents, method: string): Promise<any> {
  for (let event = await events.next(); event !== null; event = await events.next()) {
    if (event.method === method) return event.params;
  }
  return null;
}

for (const backend of BACKENDS) {
  const webkit = backend === "webkit";
  describe(`WebKitSession [${backend}]`, () => {
    let browser: Browser;

    beforeAll(async () => {
      browser = await launchForBackend(backend);
    });

    afterAll(async () => {
      await browser.close();
    });

    it("send reaches the live target and events stream in wire order", async () => {
      const page = await browser.newPage();
      const ctx = page.context();
      await page.goto("data:text/html,<title>webkit-napi</title>");
      if (!webkit) {
        expect(() => ctx.newWebKitSession(page)).toThrow(/only available on the webkit backend/);
        return;
      }
      const session = ctx.newWebKitSession(page);
      expect(typeof session.targetId()).toBe("string");
      const title = await session.send("Runtime.evaluate", { expression: "document.title", returnByValue: true });
      expect(title.result.value).toBe("webkit-napi");

      await session.send("Console.enable");
      const events = session.events();
      await session.send("Runtime.evaluate", { expression: "console.log('first'), console.log('second'), 1" });
      expect((await nextNamed(events, "Console.messageAdded")).message.text).toBe("first");
      expect((await nextNamed(events, "Console.messageAdded")).message.text).toBe("second");
    });

    it("a closed page answers nothing and its events end", async () => {
      if (!webkit) return;
      const page = await browser.newPage();
      const session = page.context().newWebKitSession(page);
      const events = session.events();
      await page.close();
      await expect(session.send("Runtime.evaluate", { expression: "1" })).rejects.toThrow();
      expect(await events.next()).toBe(null);
      expect(await session.events().next()).toBe(null);
    });

    it("a page of another context is refused", async () => {
      if (!webkit) return;
      const context = await browser.newContext();
      const other = await browser.newContext();
      const page = await context.newPage();
      expect(() => other.newWebKitSession(page)).toThrow(/the page belongs to another browser context/);
      await other.close();
      await context.close();
    });
  });
}
