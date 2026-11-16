// The raw WebKit Inspector Protocol session (context.newWebKitSession):
// on WebKit its commands reach the page's live main target, the layer
// tree is readable through it, its events stream in wire order, a
// cross-process navigation moves it to the new target, and a closed page
// answers nothing. Every other engine refuses it.

import { test, describe, expect } from '@ferridriver/test';
import type { WebKitEvents, WebKitSession } from '@ferridriver/test';

const LAYERS =
  'data:text/html,' +
  encodeURIComponent(
    '<title>Layers</title>' +
      '<div id=box style="will-change: transform; width: 40px; height: 40px; background: red">box</div>' +
      '<p id=text>text</p>',
  );

async function evaluated(session: WebKitSession, expression: string): Promise<unknown> {
  const reply = (await session.send('Runtime.evaluate', { expression, returnByValue: true })) as {
    result: { value: unknown };
  };
  return reply.result.value;
}

async function nodeOf(session: WebKitSession, root: number, selector: string): Promise<number> {
  const found = (await session.send('DOM.querySelector', { nodeId: root, selector })) as { nodeId: number };
  return found.nodeId;
}

async function nextNamed(events: WebKitEvents, method: string): Promise<Record<string, unknown> | null> {
  for (let event = await events.next(); event !== null; event = await events.next()) {
    if (event.method === method) {
      return event.params as Record<string, unknown>;
    }
  }
  return null;
}

async function refusal(action: () => unknown): Promise<string> {
  try {
    await action();
  } catch (e) {
    return String(e);
  }
  return '';
}

describe('webkit session', () => {
  test('webkit_session_reads_the_live_target_and_its_layer_tree', async ({ page, context, browserName }) => {
    await page.goto(LAYERS);
    if (browserName !== 'webkit') {
      const refused = await refusal(() => context.newWebKitSession(page));
      expect(refused.includes('only available on the webkit backend')).toBe(true);
      return;
    }
    const session = context.newWebKitSession(page);
    expect(await evaluated(session, 'document.title')).toBe('Layers');
    const document = (await session.send('DOM.getDocument', {})) as { root: { nodeId: number } };
    const root = document.root.nodeId;
    const boxed = await nodeOf(session, root, '#box');
    const text = await nodeOf(session, root, '#text');
    await session.send('LayerTree.enable', {});
    const reply = (await session.send('LayerTree.layersForNode', { nodeId: root })) as {
      layers: Array<{ nodeId?: number; memory?: number }>;
    };
    const ownedBy = (owner: number) => reply.layers.filter((layer) => layer.nodeId === owner).length;
    expect(ownedBy(boxed)).toBe(1);
    expect(ownedBy(text)).toBe(0);
    expect(reply.layers.every((layer) => typeof layer.memory === 'number')).toBe(true);
    const refused = await refusal(() => session.send('LayerTree.noSuchMethod', {}));
    expect(refused.length > 0).toBe(true);
  });

  test('webkit_session_streams_the_target_s_events_in_wire_order', async ({ page, context, browserName }) => {
    if (browserName !== 'webkit') return;
    await page.goto(LAYERS);
    const session = context.newWebKitSession(page);
    await session.send('Console.enable', {});
    const events = session.events();
    await evaluated(session, "console.log('first'), console.log('second'), 1");
    const first = await nextNamed(events, 'Console.messageAdded');
    const second = await nextNamed(events, 'Console.messageAdded');
    expect((first?.message as { text: string }).text).toBe('first');
    expect((second?.message as { text: string }).text).toBe('second');
  });

  test('webkit_session_follows_the_page_across_a_cross_process_navigation', async ({
    page,
    context,
    browserName,
    baseURL,
  }) => {
    if (browserName !== 'webkit') return;
    await page.goto(`${baseURL}/fx/landed`);
    const session = context.newWebKitSession(page);
    const before = session.targetId();
    const oldEvents = session.events();
    await page.goto(`${baseURL!.replace('127.0.0.1', 'localhost')}/fx/landed`);
    expect(session.targetId() !== before).toBe(true);
    expect(await evaluated(session, 'location.hostname')).toBe('localhost');
    let event = await oldEvents.next();
    while (event !== null) {
      event = await oldEvents.next();
    }
    expect(event).toBe(null);
  });

  test('webkit_session_of_a_closed_page_answers_nothing', async ({ page, context, browserName }) => {
    if (browserName !== 'webkit') return;
    await page.goto(LAYERS);
    const session = context.newWebKitSession(page);
    const events = session.events();
    await page.close();
    const refused = await refusal(() => evaluated(session, '1'));
    expect(refused.length > 0).toBe(true);
    expect(await events.next()).toBe(null);
    expect(await session.events().next()).toBe(null);
  });

  test('webkit_session_is_opened_only_through_the_page_s_own_context', async ({ browser, browserName }) => {
    if (browserName !== 'webkit') return;
    const context = await browser.newContext();
    const other = await browser.newContext();
    const page = await context.newPage();
    const refused = await refusal(() => other.newWebKitSession(page));
    expect(refused.includes('the page belongs to another browser context')).toBe(true);
    await other.close();
    await context.close();
  });
});
