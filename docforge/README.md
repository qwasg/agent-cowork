# DocForge-Core

文档实时编译/预览引擎 (Windows)。支持 `.docx` / `.pptx` 两种 doc-type。

## 架构

唯一事实源是 Yjs (IR 即文档)。唯一写入口是 `@docforge/doc-core` 的 mutation API,UI 与 agent 都只调它,不直接碰 Yjs/OOXML。compile/preview 只 `observe`(只读),绝不反写文档。

```
packages/
  doc-core/        # IR schema(word/ppt) + Yjs 绑定 + mutation/observe API + 序列化 + content-hash
  compile-engine/  # IR→OOXML(docx/pptxgenjs),job queue(debounce/cancel/hash-cache)
  preview-engine/  # L1 IR→DOM;L2 soffice→pdf→png 缩略缓存(可插拔渲染 + 自搓 fallback)
  sync-server/     # localhost y-websocket room
  ui/              # React 编辑器(word + ppt 双模式)
  shell/           # Tauri 2 app,拉起 sidecar(sync/compile/soffice)
```

## 开发

```bash
corepack enable pnpm   # 或 npm i -g pnpm
pnpm install
pnpm test              # 跑全部单测
pnpm demo:doc-core     # 跑 doc-core demo
```

## 对外契约 (doc-core)

```
read_document() / get_outline()
insert_block(afterId, node) / replace_text(rangeId, text) / apply_style(rangeId, style)
add_slide(index, layout) / edit_element(slideId, elId, props) / move_element(slideId, elId, geo)
export(format) -> filepath
observe(cb)
```

## 性能预算

- L1 预览延迟 < 16ms
- L2 保真编译 < 800ms(单页 / 单 slide)
- mutation → UI/preview 可见 < 100ms
- 并发写同一 doc 无丢失(CRDT)
- 导出 .docx/.pptx 可被 MS Office 正常打开
