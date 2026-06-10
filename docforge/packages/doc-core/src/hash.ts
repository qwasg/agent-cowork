/**
 * 稳定 content-hash。浏览器/Node 通用(纯 JS,无 node:crypto 依赖)。
 * 用于编译/预览缓存:相同 IR -> 相同 hash -> 命中缓存跳过编译。
 */

/** 稳定序列化:对象键排序,保证语义相同的 IR 产出相同字符串。 */
export function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value) ?? "null";
  }
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  const obj = value as Record<string, unknown>;
  const keys = Object.keys(obj).sort();
  const parts = keys
    .filter((k) => obj[k] !== undefined)
    .map((k) => `${JSON.stringify(k)}:${stableStringify(obj[k])}`);
  return `{${parts.join(",")}}`;
}

/** FNV-1a 64-bit (用 BigInt),返回 16 位十六进制字符串。 */
export function fnv1a64(str: string): string {
  const FNV_OFFSET = 0xcbf29ce484222325n;
  const FNV_PRIME = 0x100000001b3n;
  const MASK = 0xffffffffffffffffn;
  let hash = FNV_OFFSET;
  for (let i = 0; i < str.length; i++) {
    hash ^= BigInt(str.charCodeAt(i) & 0xff);
    hash = (hash * FNV_PRIME) & MASK;
    // 处理多字节字符的高位
    const hi = str.charCodeAt(i) >> 8;
    if (hi) {
      hash ^= BigInt(hi);
      hash = (hash * FNV_PRIME) & MASK;
    }
  }
  return hash.toString(16).padStart(16, "0");
}

/** 对任意 IR / 值计算稳定哈希。 */
export function contentHash(value: unknown): string {
  return fnv1a64(stableStringify(value));
}
