import { describe, it, expect } from "vitest";
import { DocCore } from "@docforge/doc-core";
import { DebounceQueue, createCompileQueue } from "../src/index.js";

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface FakeJob {
  v: number;
}

describe("DebounceQueue 三件套", () => {
  it("debounce:窗口内多次提交只跑一次,取最新", async () => {
    let runs = 0;
    const seen: number[] = [];
    const q = new DebounceQueue<FakeJob, number>({
      debounceMs: 40,
      keyOf: (j) => `k${j.v}`,
      run: async (j) => {
        runs++;
        seen.push(j.v);
        return j.v * 10;
      },
    });
    const p1 = q.schedule({ v: 1 });
    const p2 = q.schedule({ v: 2 });
    const p3 = q.schedule({ v: 3 });
    const [r1, r2, r3] = await Promise.all([p1, p2, p3]);
    expect(runs).toBe(1);
    expect(seen).toEqual([3]); // 仅最新
    expect(r1.result).toBe(30);
    expect(r2.result).toBe(30);
    expect(r3.result).toBe(30);
    expect(q.stats.debounced).toBeGreaterThanOrEqual(2);
  });

  it("hash-cache:相同 key 第二次命中缓存,不重编", async () => {
    let runs = 0;
    const q = new DebounceQueue<FakeJob, number>({
      debounceMs: 20,
      keyOf: (j) => `k${j.v}`,
      run: async (j) => {
        runs++;
        return j.v;
      },
    });
    const a = await q.schedule({ v: 7 });
    expect(a.cached).toBe(false);
    const b = await q.schedule({ v: 7 });
    expect(b.cached).toBe(true);
    expect(runs).toBe(1);
    expect(q.stats.cacheHits).toBe(1);
  });

  it("cancel:在飞任务被新提交取消,abort 触发,最终用新结果兑现", async () => {
    let aborted = false;
    let runs = 0;
    const q = new DebounceQueue<FakeJob, number>({
      debounceMs: 20,
      keyOf: (j) => `k${j.v}`,
      run: async (j, signal) => {
        runs++;
        if (j.v === 1) {
          // 慢任务,期间被取消
          signal.addEventListener("abort", () => {
            aborted = true;
          });
          await wait(200);
          return j.v;
        }
        return j.v;
      },
    });

    const p1 = q.schedule({ v: 1 });
    await wait(40); // 让 v1 进入在飞
    const p2 = q.schedule({ v: 2 }); // 触发取消 v1
    const [r1, r2] = await Promise.all([p1, p2]);
    expect(aborted).toBe(true);
    expect(q.stats.cancels).toBe(1);
    expect(r1.result).toBe(2); // 旧等待者拿到新结果
    expect(r2.result).toBe(2);
    expect(runs).toBe(2);
  });
});

describe("createCompileQueue 集成", () => {
  it("真实编译 + 缓存命中跳过", async () => {
    const d = new DocCore({ type: "word" });
    d.insert_block(null, { type: "paragraph", text: "队列编译" });
    const q = createCompileQueue({ debounceMs: 20 });

    const r1 = await q.schedule({ ir: d.read_document(), format: "docx" });
    expect(r1.cached).toBe(false);
    expect(r1.result[0]).toBe(0x50); // PK

    const r2 = await q.schedule({ ir: d.read_document(), format: "docx" });
    expect(r2.cached).toBe(true);
    expect(q.stats.compiles).toBe(1);
  });
});
