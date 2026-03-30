import { MouseEvent, UIEvent, useEffect, useMemo, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type ClipboardKind = "text" | "image" | "files";

type ClipboardRecord = {
  id: string;
  kind: ClipboardKind;
  content: string;
  searchable_text: string;
  created_at: string;
  favorite: boolean;
  tags: string[];
  image_path?: string | null;
  image_width?: number | null;
  image_height?: number | null;
  file_paths: string[];
};

type ClipboardUpdatedPayload = {
  entries: ClipboardRecord[];
  max_entries: number;
};

const appWindow = getCurrentWindow();

function MinimizeIcon() {
  return (
    <svg aria-hidden="true" className="title-icon" viewBox="0 0 16 16">
      <path
        d="M3.5 8.5h9"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.6"
      />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg aria-hidden="true" className="title-icon" viewBox="0 0 16 16">
      <path
        d="M4.25 4.25 11.75 11.75M11.75 4.25l-7.5 7.5"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.6"
      />
    </svg>
  );
}

const formatTimestamp = (value: string) =>
  new Intl.DateTimeFormat("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(value));

const escapeRegExp = (value: string) =>
  value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function highlightText(content: string, query: string) {
  const trimmed = query.trim();
  if (!trimmed) {
    return content;
  }

  const matcher = new RegExp(`(${escapeRegExp(trimmed)})`, "ig");
  return content.split(matcher).map((part, index) =>
    part.toLowerCase() === trimmed.toLowerCase() ? (
      <mark key={`${part}-${index}`}>{part}</mark>
    ) : (
      <span key={`${part}-${index}`}>{part}</span>
    )
  );
}

const fileNameFromPath = (value: string) => {
  const segments = value.split(/[\\/]/);
  return segments[segments.length - 1] || value;
};

function App() {
  const [records, setRecords] = useState<ClipboardRecord[]>([]);
  const [query, setQuery] = useState("");
  const [maxEntries, setMaxEntries] = useState(100);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [shortcut, setShortcut] = useState("Ctrl+Shift+Space");
  const [isMaximized, setIsMaximized] = useState(false);
  const [isToolbarPinned, setIsToolbarPinned] = useState(false);

  useEffect(() => {
    let isMounted = true;

    const bootstrap = async () => {
      const [initialRecords, limit, shortcutValue, maximized] = await Promise.all([
        invoke<ClipboardRecord[]>("get_clipboard_history"),
        invoke<number>("get_max_entries"),
        invoke<string>("get_toggle_shortcut"),
        appWindow.isMaximized()
      ]);

      if (isMounted) {
        setRecords(initialRecords);
        setMaxEntries(limit);
        setShortcut(shortcutValue);
        setIsMaximized(maximized);
      }
    };

    bootstrap().catch(console.error);

    const unlistenClipboard = listen<ClipboardUpdatedPayload>(
      "clipboard-updated",
      (event) => {
        if (!isMounted) {
          return;
        }

        setRecords(event.payload.entries);
        setMaxEntries(event.payload.max_entries);
      }
    );

    const unlistenResize = appWindow.listen("tauri://resize", async () => {
      if (isMounted) {
        setIsMaximized(await appWindow.isMaximized());
      }
    });

    return () => {
      isMounted = false;
      unlistenClipboard.then((fn) => fn()).catch(console.error);
      unlistenResize.then((fn) => fn()).catch(console.error);
    };
  }, []);

  const filteredRecords = useMemo(() => {
    const trimmed = query.trim().toLowerCase();
    const sorted = [...records].sort((left, right) => {
      if (left.favorite === right.favorite) {
        return (
          new Date(right.created_at).getTime() -
          new Date(left.created_at).getTime()
        );
      }

      return left.favorite ? -1 : 1;
    });

    if (!trimmed) {
      return sorted;
    }

    return sorted.filter((record) => {
      const inContent = record.content.toLowerCase().includes(trimmed);
      const inSearch = record.searchable_text.toLowerCase().includes(trimmed);
      const inTags = record.tags.some((tag) =>
        tag.toLowerCase().includes(trimmed)
      );
      return inContent || inSearch || inTags;
    });
  }, [query, records]);

  const favoriteCount = records.filter((record) => record.favorite).length;

  const handleCopy = async (id: string) => {
    await invoke("copy_record_to_clipboard", { id });
    setCopiedId(id);
    window.setTimeout(() => {
      setCopiedId((current) => (current === id ? null : current));
    }, 1400);
  };

  const handleToggleFavorite = async (id: string) => {
    const updated = await invoke<ClipboardRecord[]>("toggle_favorite", { id });
    setRecords(updated);
  };

  const handleDelete = async (id: string) => {
    const updated = await invoke<ClipboardRecord[]>("delete_record", { id });
    setRecords(updated);
    setCopiedId((current) => (current === id ? null : current));
  };

  const handleLimitChange = async (value: number) => {
    const updatedRecords = await invoke<ClipboardRecord[]>("set_max_entries", {
      limit: value
    });
    setRecords(updatedRecords);
    setMaxEntries(value);
  };

  const handleTitlebarDoubleClick = async () => {
    await appWindow.toggleMaximize();
    setIsMaximized(await appWindow.isMaximized());
  };

  const onDragMouseDown = (event: MouseEvent<HTMLElement>) => {
    if (event.button !== 0) {
      return;
    }

    const target = event.target as HTMLElement;
    if (target.closest("button, input")) {
      return;
    }

    void appWindow.startDragging();
  };

  const handleHideWindow = (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    void appWindow.hide().catch(console.error);
  };

  const handleQuitApp = (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
    void invoke("quit_app").catch(console.error);
  };

  const handleRowCopy = (id: string) => {
    void handleCopy(id);
  };

  const stopRowAction = (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation();
  };

  const handleListScroll = (event: UIEvent<HTMLElement>) => {
    const nextPinned = event.currentTarget.scrollTop > 18;
    setIsToolbarPinned((current) => (current === nextPinned ? current : nextPinned));
  };

  return (
    <main className="window-shell split-layout">
      <header className="titlebar" onDoubleClick={() => void handleTitlebarDoubleClick()}>
        <div className="titlebar-drag-zone" onMouseDown={onDragMouseDown}>
          <div className="titlebar-brand">
            <span className="title-label">Magic Pocket</span>
            <span className="title-hint">快捷唤起 {shortcut}</span>
          </div>
        </div>

        <div className="titlebar-spacer" onMouseDown={onDragMouseDown} />

        <div className="titlebar-actions">
          <button
            aria-label="隐藏应用"
            className="icon-button"
            onClick={handleHideWindow}
            type="button"
            title="隐藏"
          >
            <MinimizeIcon />
          </button>
          <button
            aria-label="退出应用"
            className="icon-button danger"
            onClick={handleQuitApp}
            type="button"
            title="退出"
          >
            <CloseIcon />
          </button>
        </div>
      </header>

      <section className={`compact-toolbar fixed-toolbar ${isToolbarPinned ? "pinned" : ""}`}>
        <div className="compact-meta">
          <strong>{filteredRecords.length}</strong>
          <span>记录</span>
          <em>{favoriteCount} 收藏</em>
          <b className={`status-pill ${isMaximized ? "wide" : ""}`}>
            {copiedId ? "已回贴" : "单击列表项回贴"}
          </b>
        </div>

        <label className="search-box">
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索内容、文件名、标签"
          />
        </label>

        <label className="limit-box">
          <span>历史 {maxEntries}</span>
          <input
            type="range"
            min={20}
            max={200}
            step={10}
            value={maxEntries}
            onChange={(event) => handleLimitChange(Number(event.target.value))}
          />
        </label>
      </section>

      <section className="records-list" onScroll={handleListScroll}>
        {filteredRecords.map((record) => (
          <button
            className={`record-row ${copiedId === record.id ? "active" : ""}`}
            key={record.id}
            onClick={() => handleRowCopy(record.id)}
            type="button"
          >
            <div className="record-row-top">
              <div className="record-top-meta">
                <span className="record-kind">
                  {record.kind === "text"
                    ? "文本"
                    : record.kind === "image"
                      ? "图片"
                      : "文件"}
                </span>
                <span className="record-time">{formatTimestamp(record.created_at)}</span>
              </div>

              <div className="record-actions-inline">
                <button
                  className={`star-button compact ${record.favorite ? "active" : ""}`}
                  onClick={(event) => {
                    stopRowAction(event);
                    void handleToggleFavorite(record.id);
                  }}
                  type="button"
                >
                  {record.favorite ? "已藏" : "收藏"}
                </button>
                <button
                  className="delete-button compact"
                  onClick={(event) => {
                    stopRowAction(event);
                    void handleDelete(record.id);
                  }}
                  type="button"
                >
                  删除
                </button>
              </div>
            </div>

            {record.kind === "image" && record.image_path ? (
              <div className="record-preview-media">
                <img
                  alt={record.content}
                  className="record-thumb"
                  src={convertFileSrc(record.image_path)}
                />
                <div className="record-preview-scroll image-copy">
                  <p className="record-content">{record.content}</p>
                  <span className="record-dimensions">
                    {record.image_width} x {record.image_height}
                  </span>
                </div>
              </div>
            ) : record.kind === "files" ? (
              <div className="record-preview-scroll file-stack">
                {record.file_paths.map((path) => (
                  <div className="file-item compact" key={`${record.id}-${path}`}>
                    <strong>{highlightText(fileNameFromPath(path), query)}</strong>
                    <span>{path}</span>
                  </div>
                ))}
              </div>
            ) : (
              <div className="record-preview-scroll text-preview">
                <p className="record-content">{highlightText(record.content, query)}</p>
              </div>
            )}

            {record.tags.length > 0 ? (
              <div className="tag-row compact">
                {record.tags.slice(0, 4).map((tag) => (
                  <span className="tag-chip readonly" key={`${record.id}-${tag}`}>
                    #{tag}
                  </span>
                ))}
              </div>
            ) : null}
          </button>
        ))}

        {filteredRecords.length === 0 ? (
          <div className="empty-state">
            <h2>没有匹配记录</h2>
            <p>复制文本、图片或文件后，这里会自动更新。</p>
          </div>
        ) : null}
      </section>
    </main>
  );
}

export default App;
