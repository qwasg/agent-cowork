/**
 * @docforge/doc-core
 *
 * 文档子系统的唯一事实源与对外契约。IR 即文档(Yjs),
 * UI 与 agent 都只调这套 mutation/observe API;compile/preview 只读。
 */

export * from "./types.js";
export { DocCore, LOCAL_ORIGIN } from "./doc-core.js";
export type { DocCoreOptions } from "./doc-core.js";
export { contentHash, stableStringify, fnv1a64 } from "./hash.js";
export { defaultIdFactory, createSeqIdFactory } from "./ids.js";
export type { IdFactory } from "./ids.js";
export { LAYOUTS, getLayoutSeeds, SLIDE_WIDTH, SLIDE_HEIGHT } from "./layouts.js";
export type { SeedElement } from "./layouts.js";
export { toJSON, fromJSON, applyIRToDoc } from "./serialize.js";
export * as WordBinding from "./binding/word.js";
export * as PptBinding from "./binding/ppt.js";
