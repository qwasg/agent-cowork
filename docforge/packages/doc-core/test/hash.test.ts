import { describe, it, expect } from "vitest";
import { contentHash, stableStringify } from "../src/index.js";

describe("contentHash / stableStringify", () => {
  it("键顺序无关", () => {
    expect(stableStringify({ a: 1, b: 2 })).toBe(stableStringify({ b: 2, a: 1 }));
    expect(contentHash({ a: 1, b: 2 })).toBe(contentHash({ b: 2, a: 1 }));
  });

  it("内容不同则 hash 不同", () => {
    expect(contentHash({ a: 1 })).not.toBe(contentHash({ a: 2 }));
  });

  it("忽略 undefined 字段", () => {
    expect(stableStringify({ a: 1, b: undefined })).toBe(stableStringify({ a: 1 }));
  });

  it("hash 为 16 位十六进制且稳定", () => {
    const h = contentHash({ hello: "world", n: [1, 2, 3] });
    expect(h).toMatch(/^[0-9a-f]{16}$/);
    expect(h).toBe(contentHash({ n: [1, 2, 3], hello: "world" }));
  });

  it("嵌套数组顺序敏感", () => {
    expect(contentHash([1, 2, 3])).not.toBe(contentHash([3, 2, 1]));
  });
});
