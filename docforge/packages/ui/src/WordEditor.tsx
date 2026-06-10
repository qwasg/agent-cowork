import { useEditor, EditorContent } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Underline from "@tiptap/extension-underline";
import Collaboration from "@tiptap/extension-collaboration";
import type { DocClient } from "./docClient.js";

/**
 * Word 编辑器:Tiptap(ProseMirror)+ y-prosemirror Collaboration。
 * 绑定到 doc-core 的同一 Y.Doc 的 "word" fragment —— agent 经 doc-core 写入时也会反映到这里。
 */
export function WordEditor({ client }: { client: DocClient }) {
  const editor = useEditor({
    extensions: [
      StarterKit.configure({ history: false }), // 协同用 Yjs 的 UndoManager,关掉本地 history
      Underline,
      Collaboration.configure({ document: client.doc, field: "word" }),
    ],
    editorProps: {
      attributes: { class: "word-surface", "data-testid": "word-editor" },
    },
  });

  if (!editor) return <div className="word-surface">加载中…</div>;

  return (
    <div className="word-editor">
      <div className="toolbar">
        <button
          data-testid="btn-bold"
          className={editor.isActive("bold") ? "active" : ""}
          onClick={() => editor.chain().focus().toggleBold().run()}
        >
          B
        </button>
        <button
          className={editor.isActive("italic") ? "active" : ""}
          onClick={() => editor.chain().focus().toggleItalic().run()}
        >
          <i>I</i>
        </button>
        <button
          className={editor.isActive("underline") ? "active" : ""}
          onClick={() => editor.chain().focus().toggleUnderline().run()}
        >
          <u>U</u>
        </button>
        <span className="sep" />
        <button onClick={() => editor.chain().focus().toggleHeading({ level: 1 }).run()}>H1</button>
        <button onClick={() => editor.chain().focus().toggleHeading({ level: 2 }).run()}>H2</button>
        <button onClick={() => editor.chain().focus().setParagraph().run()}>P</button>
      </div>
      <EditorContent editor={editor} />
    </div>
  );
}
