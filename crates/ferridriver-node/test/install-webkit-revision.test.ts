// NAPI coverage for installWebkitRevision: a pinned WebKit build lands in
// the browser cache Playwright lays out, from the download hosts it is
// given, marked complete, and is kept rather than fetched again; a
// revision that is not a build number is refused before any download.
// A local server stands in for Playwright's CDN.

import { describe, it, expect, beforeAll, afterAll } from "bun:test";
import { existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { installWebkitRevision } from "../index.js";

type Entry = { name: string; data?: Uint8Array; directory?: boolean };

function storedZip(entries: Entry[]): Uint8Array {
  const local: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = new TextEncoder().encode(entry.name);
    const data = entry.data ?? new Uint8Array();
    const crc = Bun.hash.crc32(data) >>> 0;
    const mode = entry.directory ? 0o040755 : 0o100644;
    const head = new DataView(new ArrayBuffer(30));
    head.setUint32(0, 0x04034b50, true);
    head.setUint16(4, 20, true);
    head.setUint32(14, crc, true);
    head.setUint32(18, data.length, true);
    head.setUint32(22, data.length, true);
    head.setUint16(26, name.length, true);
    local.push(new Uint8Array(head.buffer), name, data);
    const record = new DataView(new ArrayBuffer(46));
    record.setUint32(0, 0x02014b50, true);
    record.setUint16(4, (3 << 8) | 20, true);
    record.setUint16(6, 20, true);
    record.setUint32(16, crc, true);
    record.setUint32(20, data.length, true);
    record.setUint32(24, data.length, true);
    record.setUint16(28, name.length, true);
    record.setUint32(38, ((mode << 16) >>> 0) + (entry.directory ? 0x10 : 0), true);
    record.setUint32(42, offset, true);
    central.push(new Uint8Array(record.buffer), name);
    offset += 30 + name.length + data.length;
  }
  const centralSize = central.reduce((total, chunk) => total + chunk.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, entries.length, true);
  end.setUint16(10, entries.length, true);
  end.setUint32(12, centralSize, true);
  end.setUint32(16, offset, true);
  const parts = [...local, ...central, new Uint8Array(end.buffer)];
  const archive = new Uint8Array(parts.reduce((total, part) => total + part.length, 0));
  let at = 0;
  for (const part of parts) {
    archive.set(part, at);
    at += part.length;
  }
  return archive;
}

const BUILD = storedZip([
  { name: "pw_run.sh", data: new TextEncoder().encode("#!/bin/sh\n") },
  { name: "minibrowser-wpe/", directory: true },
]);

describe("installWebkitRevision", () => {
  let server: ReturnType<typeof Bun.serve>;
  let host: string;
  const asked: string[] = [];
  const folders: string[] = [];

  const folder = () => {
    const made = mkdtempSync(join(tmpdir(), "ferridriver-webkit-revision-"));
    folders.push(made);
    return made;
  };

  beforeAll(() => {
    server = Bun.serve({
      port: 0,
      fetch(request) {
        const path = new URL(request.url).pathname;
        asked.push(path);
        if (path.startsWith("/builds/webkit/2370/") && path.endsWith(".zip")) {
          return new Response(BUILD);
        }
        return new Response("no such build", { status: 404 });
      },
    });
    host = `http://127.0.0.1:${server.port}`;
  });

  afterAll(() => {
    server.stop(true);
    for (const made of folders) rmSync(made, { recursive: true, force: true });
  });

  it("installs the revision into the cache it is given, marked complete", async () => {
    const cache = folder();
    const installed = await installWebkitRevision("2370", { cacheDir: cache, downloadHosts: [host] });
    const build = join(cache, "webkit-2370");
    expect(installed).toBe(join(build, "pw_run.sh"));
    expect(existsSync(join(build, "INSTALLATION_COMPLETE"))).toBe(true);
    expect(statSync(join(build, "minibrowser-wpe")).isDirectory()).toBe(true);
  });

  it("keeps a build already marked complete without downloading it again", async () => {
    const cache = folder();
    await installWebkitRevision("2370", { cacheDir: cache, downloadHosts: [host] });
    const before = asked.length;
    const kept = await installWebkitRevision("2370", { cacheDir: cache, downloadHosts: [host] });
    expect(kept).toBe(join(cache, "webkit-2370", "pw_run.sh"));
    expect(asked.length).toBe(before);
  });

  it("refuses a revision that is not a build number before any download", async () => {
    const before = asked.length;
    await expect(
      installWebkitRevision("latest", { cacheDir: folder(), downloadHosts: [host] }),
    ).rejects.toThrow(/not a WebKit build number/);
    expect(asked.length).toBe(before);
  });

  it("installs into Playwright's own cache when no cacheDir is named", async () => {
    // The addon reads PLAYWRIGHT_BROWSERS_PATH from the process
    // environment, which `process.env` writes in this process do not
    // reach, so the install runs in a child given its own environment.
    const cache = folder();
    const addon = JSON.stringify(join(import.meta.dir, "..", "index.js"));
    const child = Bun.spawn(
      [
        process.execPath,
        "-e",
        `import(${addon}).then((addon) => addon.installWebkitRevision("2370", { downloadHosts: [${JSON.stringify(host)}] })).then((path) => process.stdout.write(path))`,
      ],
      { env: { ...process.env, PLAYWRIGHT_BROWSERS_PATH: cache }, stdout: "pipe", stderr: "pipe" },
    );
    const installed = await new Response(child.stdout).text();
    const stderr = await new Response(child.stderr).text();
    expect(await child.exited, stderr).toBe(0);
    expect(installed).toBe(join(cache, "webkit-2370", "pw_run.sh"));
    expect(existsSync(join(cache, "webkit-2370", "INSTALLATION_COMPLETE"))).toBe(true);
  });

  it("fails when no host serves the revision", async () => {
    await expect(
      installWebkitRevision("2371", { cacheDir: folder(), downloadHosts: [host] }),
    ).rejects.toThrow(/download failed/);
  });
});
