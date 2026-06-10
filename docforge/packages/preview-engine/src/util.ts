/** HTML/XML 文本转义。 */
export function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** 颜色规范化:接受 "RRGGBB" 或 "#RRGGBB",输出 "#RRGGBB"。 */
export function color(c: unknown, dflt = "#222222"): string {
  if (typeof c !== "string" || !c) return dflt;
  return c.startsWith("#") ? c : `#${c}`;
}
