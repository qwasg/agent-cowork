// dev/test 用:先注册 tsx(处理 .ts 与 .js->.ts 回退),再加载 TS worker。
// 生产(已编译为 JS)直接把 sidecar 的 workerUrl 指向 worker.js,无需此引导。
import { register } from "tsx/esm/api";
register();
await import("./worker.ts");
