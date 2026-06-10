import { useRef, useState } from "react";
import { SLIDE_WIDTH, SLIDE_HEIGHT, type SlideElement } from "@docforge/doc-core";
import type { DocClient } from "./docClient.js";
import { useDocIR } from "./hooks.js";

const PX_PER_INCH = 80;

/**
 * PPT 编辑器:SVG 渲染 Y.Array<slide>。
 * 拖动元素 -> move_element;双击文本 -> edit_element。全部经 doc-core 契约。
 */
export function PptEditor({ client }: { client: DocClient }) {
  const ir = useDocIR(client.core);
  const slides = ir.type === "ppt" ? ir.slides : [];
  const [current, setCurrent] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [layout, setLayout] = useState("titleBody");

  const slide = slides[current];
  const dragRef = useRef<{ id: string; startX: number; startY: number; geoX: number; geoY: number } | null>(null);

  function addSlide() {
    const id = client.core.add_slide(slides.length, layout);
    setCurrent(slides.length);
    setSelected(null);
    return id;
  }

  function onPointerDown(e: React.PointerEvent, el: SlideElement) {
    if (editing) return;
    setSelected(el.id);
    (e.target as Element).setPointerCapture?.(e.pointerId);
    dragRef.current = {
      id: el.id,
      startX: e.clientX,
      startY: e.clientY,
      geoX: el.geo.x,
      geoY: el.geo.y,
    };
  }

  function onPointerMove(e: React.PointerEvent) {
    const d = dragRef.current;
    if (!d || !slide) return;
    const dx = (e.clientX - d.startX) / PX_PER_INCH;
    const dy = (e.clientY - d.startY) / PX_PER_INCH;
    client.core.move_element(slide.id, d.id, {
      x: Math.round((d.geoX + dx) * 100) / 100,
      y: Math.round((d.geoY + dy) * 100) / 100,
    });
  }

  function onPointerUp() {
    dragRef.current = null;
  }

  return (
    <div className="ppt-editor">
      <div className="ppt-sidebar" data-testid="ppt-sidebar">
        <div className="ppt-controls">
          <select value={layout} onChange={(e) => setLayout(e.target.value)}>
            <option value="title">title</option>
            <option value="titleBody">titleBody</option>
            <option value="twoContent">twoContent</option>
            <option value="blank">blank</option>
          </select>
          <button data-testid="btn-add-slide" onClick={addSlide}>
            + 幻灯片
          </button>
        </div>
        <ol className="slide-list">
          {slides.map((s, i) => (
            <li
              key={s.id}
              className={i === current ? "active" : ""}
              onClick={() => {
                setCurrent(i);
                setSelected(null);
              }}
            >
              <span className="slide-no">{i + 1}</span>
              <span className="slide-layout">{s.layout}</span>
            </li>
          ))}
        </ol>
      </div>

      <div className="ppt-stage-wrap">
        {!slide ? (
          <div className="empty-hint">点击「+ 幻灯片」新建第一页</div>
        ) : (
          <svg
            className="ppt-stage"
            data-testid="ppt-stage"
            width={SLIDE_WIDTH * PX_PER_INCH}
            height={SLIDE_HEIGHT * PX_PER_INCH}
            viewBox={`0 0 ${SLIDE_WIDTH} ${SLIDE_HEIGHT}`}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
          >
            <rect x={0} y={0} width={SLIDE_WIDTH} height={SLIDE_HEIGHT} fill="#fff" stroke="#ddd" strokeWidth={0.01} />
            {slide.elements.map((el) => (
              <ElementView
                key={el.id}
                el={el}
                selected={selected === el.id}
                editing={editing === el.id}
                onPointerDown={(e) => onPointerDown(e, el)}
                onDoubleClick={() => el.type === "text" && setEditing(el.id)}
                onCommitText={(text) => {
                  client.core.edit_element(slide.id, el.id, { text });
                  setEditing(null);
                }}
              />
            ))}
          </svg>
        )}
      </div>
    </div>
  );
}

function ElementView({
  el,
  selected,
  editing,
  onPointerDown,
  onDoubleClick,
  onCommitText,
}: {
  el: SlideElement;
  selected: boolean;
  editing: boolean;
  onPointerDown: (e: React.PointerEvent) => void;
  onDoubleClick: () => void;
  onCommitText: (text: string) => void;
}) {
  const { x, y, w, h } = el.geo;
  const props = el.props as Record<string, unknown>;
  const fontSize = (Number(props.fontSize) || 18) / PX_PER_INCH;
  const color = typeof props.color === "string" ? `#${props.color}` : "#222";
  const text = typeof props.text === "string" ? props.text : "";

  return (
    <g
      transform={`translate(${x} ${y}) rotate(${el.geo.rot ?? 0})`}
      onPointerDown={onPointerDown}
      onDoubleClick={onDoubleClick}
      style={{ cursor: "move" }}
    >
      {el.type === "shape" && (
        <rect width={w} height={h} fill={typeof props.fill === "string" ? `#${props.fill}` : "#4a90d9"} rx={0.05} />
      )}
      {el.type === "image" && (
        <image href={String(props.src ?? "")} width={w} height={h} preserveAspectRatio="xMidYMid meet" />
      )}
      {el.type === "text" && (
        <foreignObject width={w} height={h}>
          {editing ? (
            <textarea
              autoFocus
              defaultValue={text}
              data-testid="text-edit"
              style={{ width: "100%", height: "100%", fontSize, border: "none", resize: "none", lineHeight: 1.2 }}
              onBlur={(e) => onCommitText(e.target.value)}
            />
          ) : (
            <div
              style={{
                width: "100%",
                height: "100%",
                fontSize,
                color,
                fontWeight: props.bold ? 700 : 400,
                fontStyle: props.italic ? "italic" : "normal",
                textAlign: (props.align as "left" | "center" | "right") ?? "left",
                overflow: "hidden",
                lineHeight: 1.2,
              }}
            >
              {text || <span style={{ opacity: 0.35 }}>文本</span>}
            </div>
          )}
        </foreignObject>
      )}
      {selected && (
        <rect width={w} height={h} fill="none" stroke="#ff5a5a" strokeWidth={0.02} strokeDasharray="0.06 0.04" />
      )}
    </g>
  );
}
