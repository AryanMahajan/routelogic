import { useMemo, useState } from "react";
import { api, CoreError } from "../api";
import type { Collection, RequestDraft } from "../types";
import { MethodBadge } from "./MethodBadge";
import { FolderRow, TreeRow } from "./Tree";

/**
 * Saved requests, as the files they live in: one collection per file, folders as a path on
 * each request, order as the order in the file.
 *
 * Everything here edits a loaded collection and writes the whole file back. That keeps the
 * core's surface tiny (save / delete / rename a collection) and means the on-disk result is
 * always exactly what is shown. Drag to reorder or move; the `…` menus do the rest.
 */
export function Collections({
  collections,
  onOpenRequest,
  onChanged,
}: {
  collections: Collection[];
  onOpenRequest: (request: RequestDraft, collection: string) => void;
  onChanged: () => void;
}) {
  // A collection starts open and its folders closed: `collapsed` holds closed collections,
  // `expanded` the folders that have been opened. `toggle` tells them apart by key.
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [dragging, setDragging] = useState<Drag | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  function toggle(key: string) {
    const flip = (current: Set<string>) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    };
    if (key.startsWith("f:")) setExpanded(flip);
    else setCollapsed(flip);
  }
  const isOpen = (key: string) => (key.startsWith("f:") ? expanded.has(key) : !collapsed.has(key));

  async function run(action: () => Promise<void>) {
    try {
      await action();
      setError(null);
      onChanged();
    } catch (e) {
      setError(e instanceof CoreError ? e.message : String(e));
    }
  }

  function newCollection() {
    const name = window.prompt("Collection name")?.trim();
    if (!name) return;
    if (collections.some((c) => c.name === name)) {
      setError(`A collection named "${name}" already exists.`);
      return;
    }
    void run(() => api.saveCollection({ version: 1, name, requests: [] }));
  }

  /** Drop `dragging` at a place inside `target`, writing every collection it touched. */
  function drop(target: DropTarget) {
    if (!dragging) return;
    const source = collections.find((c) => c.name === dragging.collection);
    const destination = collections.find((c) => c.name === target.collection);
    if (!source || !destination) return;
    const request = source.requests.find((r) => r.id === dragging.id);
    if (!request) return;

    const moved: RequestDraft = { ...request, folder: target.folder ?? null };
    const sourceRequests = source.requests.filter((r) => r.id !== dragging.id);
    const destinationRequests =
      source === destination ? sourceRequests : [...destination.requests];

    let index = destinationRequests.length;
    if (target.before) {
      const at = destinationRequests.findIndex((r) => r.id === target.before);
      if (at >= 0) index = at;
    }
    destinationRequests.splice(index, 0, moved);

    setDragging(null);
    setOver(null);
    void run(async () => {
      await api.saveCollection({ ...destination, requests: destinationRequests });
      if (source !== destination) {
        await api.saveCollection({ ...source, requests: sourceRequests });
      }
    });
  }

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between px-2 pb-2 text-[11px] text-muted">
        <span className="tabular-nums">
          {collections.reduce((n, c) => n + c.requests.length, 0)} saved
        </span>
        <button onClick={newCollection} className="transition hover:text-ink">
          + Collection
        </button>
      </div>

      {error && (
        <p className="mx-2 mb-2 rounded border border-method-delete/40 bg-method-delete/10 px-2 py-1 text-[11px] text-method-delete">
          {error}
        </p>
      )}

      {collections.length === 0 && (
        <p className="px-2 py-4 text-muted">
          No saved requests yet. Save one from the editor, or import a cURL command or an
          OpenAPI document.
        </p>
      )}

      {collections.map((collection) => (
        <CollectionBlock
          key={collection.name}
          collection={collection}
          collections={collections}
          isOpen={isOpen}
          toggle={toggle}
          dragging={dragging}
          over={over}
          setOver={setOver}
          onDragStart={(id) => setDragging({ collection: collection.name, id })}
          onDragEnd={() => {
            setDragging(null);
            setOver(null);
          }}
          onDrop={drop}
          onOpenRequest={(request) => onOpenRequest(request, collection.name)}
          run={run}
        />
      ))}
    </div>
  );
}

interface Drag {
  collection: string;
  id: string;
}

interface DropTarget {
  collection: string;
  /** Folder to file the request under; null for the top level. */
  folder: string | null;
  /** Insert before this request id; at the end when absent. */
  before?: string;
}

/** A folder tree built from the `folder` path on each request. */
interface Node {
  folders: Map<string, Node>;
  requests: RequestDraft[];
}

function tree(requests: RequestDraft[]): Node {
  const root: Node = { folders: new Map(), requests: [] };
  for (const request of requests) {
    const parts = (request.folder ?? "").split("/").filter(Boolean);
    let node = root;
    for (const part of parts) {
      let next = node.folders.get(part);
      if (!next) {
        next = { folders: new Map(), requests: [] };
        node.folders.set(part, next);
      }
      node = next;
    }
    node.requests.push(request);
  }
  return root;
}

function CollectionBlock({
  collection,
  collections,
  isOpen,
  toggle,
  dragging,
  over,
  setOver,
  onDragStart,
  onDragEnd,
  onDrop,
  onOpenRequest,
  run,
}: {
  collection: Collection;
  collections: Collection[];
  isOpen: (key: string) => boolean;
  toggle: (key: string) => void;
  dragging: Drag | null;
  over: string | null;
  setOver: (key: string | null) => void;
  onDragStart: (id: string) => void;
  onDragEnd: () => void;
  onDrop: (target: DropTarget) => void;
  onOpenRequest: (request: RequestDraft) => void;
  run: (action: () => Promise<void>) => Promise<void>;
}) {
  const root = useMemo(() => tree(collection.requests), [collection.requests]);
  const key = `c:${collection.name}`;
  const folders = useMemo(() => {
    const names = new Set<string>();
    for (const r of collection.requests) if (r.folder) names.add(r.folder);
    return [...names].sort();
  }, [collection.requests]);

  function save(requests: RequestDraft[]) {
    return run(() => api.saveCollection({ ...collection, requests }));
  }

  const collectionMenu: MenuItem[] = [
    {
      label: "Rename collection",
      onClick: () => {
        const name = window.prompt("New name", collection.name)?.trim();
        if (!name || name === collection.name) return;
        void run(() => api.renameCollection(collection.name, name));
      },
    },
    {
      label: "Delete collection",
      danger: true,
      onClick: () => {
        const n = collection.requests.length;
        if (
          window.confirm(
            `Delete "${collection.name}" and the ${n} request${n === 1 ? "" : "s"} in it?`,
          )
        ) {
          void run(() => api.deleteCollection(collection.name));
        }
      },
    },
  ];

  function requestMenu(request: RequestDraft): MenuItem[] {
    return [
      {
        label: "Rename",
        onClick: () => {
          const name = window.prompt("Request name", request.name ?? "")?.trim();
          if (!name) return;
          void save(collection.requests.map((r) => (r.id === request.id ? { ...r, name } : r)));
        },
      },
      {
        label: "Duplicate",
        onClick: () => {
          const copy: RequestDraft = {
            ...request,
            id: crypto.randomUUID(),
            name: `${request.name ?? request.url} copy`,
          };
          const at = collection.requests.findIndex((r) => r.id === request.id);
          const next = [...collection.requests];
          next.splice(at + 1, 0, copy);
          void save(next);
        },
      },
      {
        label: "Move to folder…",
        onClick: () => {
          const folder = window.prompt(
            `Folder (use / to nest; empty for the top level)${
              folders.length ? `\nExisting: ${folders.join(", ")}` : ""
            }`,
            request.folder ?? "",
          );
          if (folder === null) return;
          const cleaned = folder.split("/").map((p) => p.trim()).filter(Boolean).join("/");
          void save(
            collection.requests.map((r) =>
              r.id === request.id ? { ...r, folder: cleaned || null } : r,
            ),
          );
        },
      },
      ...collections
        .filter((c) => c.name !== collection.name)
        .map<MenuItem>((other) => ({
          label: `Move to "${other.name}"`,
          onClick: () =>
            run(async () => {
              await api.saveCollection({
                ...other,
                requests: [...other.requests, { ...request, folder: null }],
              });
              await api.saveCollection({
                ...collection,
                requests: collection.requests.filter((r) => r.id !== request.id),
              });
            }),
        })),
      {
        label: "Delete",
        danger: true,
        onClick: () => {
          if (window.confirm(`Delete "${request.name ?? request.url}"?`)) {
            void save(collection.requests.filter((r) => r.id !== request.id));
          }
        },
      },
    ];
  }

  function renderNode(node: Node, path: string, depth: number): React.ReactNode {
    return (
      <>
        {[...node.folders.entries()]
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([name, child]) => {
            const folderPath = path ? `${path}/${name}` : name;
            const folderKey = `f:${collection.name}:${folderPath}`;
            const isOver = over === folderKey;
            return (
              <div key={folderKey}>
                <FolderRow
                  depth={depth}
                  open={isOpen(folderKey)}
                  name={name}
                  count={countRequests(child)}
                  onToggle={() => toggle(folderKey)}
                  onDragOver={(e) => {
                    if (!dragging) return;
                    e.preventDefault();
                    setOver(folderKey);
                  }}
                  onDragLeave={() => isOver && setOver(null)}
                  onDrop={(e) => {
                    e.preventDefault();
                    onDrop({ collection: collection.name, folder: folderPath });
                  }}
                  className={isOver ? "bg-accent/15 ring-1 ring-accent" : ""}
                />
                {isOpen(folderKey) && renderNode(child, folderPath, depth + 1)}
              </div>
            );
          })}

        {node.requests.map((request) => {
          const rowKey = `r:${request.id}`;
          const isOver = over === rowKey;
          return (
            <TreeRow
              key={request.id}
              depth={depth}
              draggable
              onDragStart={(e) => {
                e.dataTransfer.effectAllowed = "move";
                onDragStart(request.id);
              }}
              onDragEnd={onDragEnd}
              onDragOver={(e) => {
                if (!dragging || dragging.id === request.id) return;
                e.preventDefault();
                setOver(rowKey);
              }}
              onDragLeave={() => isOver && setOver(null)}
              onDrop={(e) => {
                e.preventDefault();
                onDrop({ collection: collection.name, folder: path || null, before: request.id });
              }}
              className={`${isOver ? "shadow-[inset_0_2px_0_0_var(--color-accent)]" : ""} ${
                dragging?.id === request.id ? "opacity-40" : ""
              }`}
            >
              <button
                onClick={() => onOpenRequest(request)}
                title={request.url}
                className="flex h-full min-w-0 flex-1 items-center gap-2 pl-4 text-left"
              >
                <MethodBadge method={request.method} className="w-11 shrink-0" />
                <span className="min-w-0 flex-1 truncate">{request.name ?? request.url}</span>
              </button>
              <Menu items={requestMenu(request)} />
            </TreeRow>
          );
        })}
      </>
    );
  }

  const headerOver = over === key;

  return (
    <div className="mb-2">
      <div
        onDragOver={(e) => {
          if (!dragging) return;
          e.preventDefault();
          setOver(key);
        }}
        onDragLeave={() => headerOver && setOver(null)}
        onDrop={(e) => {
          e.preventDefault();
          onDrop({ collection: collection.name, folder: null });
        }}
        className={`rounded ${headerOver ? "bg-accent/15 ring-1 ring-accent" : ""}`}
      >
        <FolderRow
          open={isOpen(key)}
          name={collection.name}
          count={collection.requests.length}
          onToggle={() => toggle(key)}
          className="font-medium"
          trailing={<Menu items={collectionMenu} />}
        />
      </div>

      {isOpen(key) && (
        <div>
          {collection.requests.length === 0 && (
            <p className="py-1 pl-9 text-[11px] text-muted">Empty — drop a request here.</p>
          )}
          {renderNode(root, "", 1)}
        </div>
      )}
    </div>
  );
}

function countRequests(node: Node): number {
  let n = node.requests.length;
  for (const child of node.folders.values()) n += countRequests(child);
  return n;
}

interface MenuItem {
  label: string;
  onClick: () => void;
  danger?: boolean;
}

/** A `…` button revealing a small list of actions. Closes on choice or outside click. */
function Menu({ items }: { items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative shrink-0">
      <button
        onClick={(e) => {
          e.stopPropagation();
          setOpen(!open);
        }}
        title="More"
        className={`rounded px-1 text-muted transition hover:text-ink ${
          open ? "opacity-100" : "opacity-0 group-hover:opacity-100"
        }`}
      >
        …
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <ul className="absolute right-0 z-50 mt-1 min-w-40 rounded border border-edge bg-panel py-1 shadow-xl">
            {items.map((item) => (
              <li key={item.label}>
                <button
                  onClick={() => {
                    setOpen(false);
                    item.onClick();
                  }}
                  className={`block w-full whitespace-nowrap px-3 py-1 text-left transition hover:bg-raised ${
                    item.danger ? "text-method-delete" : ""
                  }`}
                >
                  {item.label}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
